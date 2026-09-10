//! `ADI.exe --supervise` — the process a scheduled task actually runs.
//!
//! Windows has no launchd. A Task Scheduler action is a command line, so a service used to be
//! started as `cmd.exe /C set VAR=… && adi-app.exe 8000 > log 2>&1`, which bought the two things
//! a command line cannot otherwise carry — an environment and a redirect — at a price nobody
//! agreed to:
//!
//!   * **A console window per service.** Four black rectangles on the desktop, and a console
//!     window is not decoration: closing it kills everything attached to it. The platform was
//!     one stray click away from being stopped, and people made that click.
//!   * **A stop that did not stop.** `schtasks /End` ends the task's own process — the `cmd` —
//!     and the service it spawned went on serving with a dead parent. "Stop ADI" left the panel
//!     answering on port 8000 and the front door still bound.
//!
//! This is the fix, and it is the same shape as launchd's: a parent that owns the child. ADI.exe
//! is a GUI-subsystem binary, so this process has no console and nothing to close. It sets the
//! environment, opens the log, and starts the real program inside a **job object** marked
//! `KILL_ON_JOB_CLOSE`; when this process ends — however it ends — Windows tears the job down
//! and the service goes with it. It then waits, and exits with the child's own exit code, so the
//! task's `<RestartOnFailure>` still sees a failure as a failure.
//!
//! Nothing here decides anything about the install: the command line it is handed comes from
//! `adi-core`'s `launchd::enable`, the one place that knows what a service is.

// Three documented kernel32 entry points (create a job, put a process in it, close it). Same
// posture as `tray.rs`: the Win32 call is the feature, and wrapping each one would hide it.
#![allow(unsafe_code)]

/// The flag that turns ADI.exe from the app into a supervisor. `adi-core` writes it into the
/// task XML; this constant is the only definition of it on this side.
pub const FLAG: &str = "--supervise";

/// What the supervisor was asked to run.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Where the child's stdout and stderr go.
    pub log: Option<String>,
    /// Environment the child is started with, on top of this process's own.
    pub env: Vec<(String, String)>,
    /// The program and its arguments, everything after `--`.
    pub program: Vec<String>,
}

/// Parse `--supervise --log <path> [--env K=V]… -- <program> [args…]`.
///
/// Hand-rolled rather than pulled from a crate: it is three options with no abbreviations, read
/// from a command line this repo also writes, and a parser is easier to keep honest than a
/// dependency is to keep small.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Args {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            FLAG => {}
            "--log" => out.log = it.next(),
            "--env" => {
                if let Some(pair) = it.next() {
                    if let Some((k, v)) = pair.split_once('=') {
                        out.env.push((k.to_string(), v.to_string()));
                    }
                }
            }
            "--" => {
                out.program.extend(it.by_ref());
                break;
            }
            _ => {}
        }
    }
    out
}

#[cfg(windows)]
pub fn main() -> i32 {
    use std::fs::{self, OpenOptions};
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, Stdio};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let args = parse(std::env::args().skip(1));
    let Some((program, rest)) = args.program.split_first() else {
        return 2;
    };

    // Appended, not truncated: this is the launchd `StandardOutPath` analog, and a service that
    // restart-on-failure has restarted four times should leave four accounts of why, not one.
    // The directory is created here as well as in adi-core, because this is the last moment
    // before the redirect that would fail without it.
    let log = args.log.as_deref().map(Path::new).and_then(|path| {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        OpenOptions::new().create(true).append(true).open(path).ok()
    });
    let (out, err) = match log {
        Some(file) => match file.try_clone() {
            Ok(dup) => (Stdio::from(file), Stdio::from(dup)),
            Err(_) => (Stdio::from(file), Stdio::null()),
        },
        None => (Stdio::null(), Stdio::null()),
    };

    let mut command = Command::new(program);
    command
        .args(rest)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .creation_flags(CREATE_NO_WINDOW);
    for (k, v) in &args.env {
        command.env(k, v);
    }

    let job = Job::create();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            // Nowhere to print: no console, and the log belongs to the child that never
            // started. The exit code is what Task Scheduler records, and non-zero is what
            // `<RestartOnFailure>` reads.
            let _ = e;
            return 1;
        }
    };
    if let Some(job) = &job {
        job.adopt(child.as_raw_handle());
    }

    // Holding `job` until here is the whole point: dropping it closes the last handle, which is
    // what kills the child. `File` and `Child` clean themselves up.
    let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
    drop(job);
    code
}

#[cfg(not(windows))]
pub fn main() -> i32 {
    2
}

/// A Windows job object that kills what it contains when the last handle to it closes.
///
/// The one Win32 primitive that makes a process tree die with its parent — the thing launchd
/// gives us for free on macOS and cgroups give systemd on Linux.
#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Job {
    fn create() -> Option<Self> {
        use std::ptr;

        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        // SAFETY: an unnamed job with default security; the call returns null on failure, which
        // is the only thing we do with the result before checking it.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return None;
        }
        // SAFETY: a zeroed struct of exactly the size we pass, matching the information class.
        // KILL_ON_JOB_CLOSE is the only limit set; everything else stays at the default.
        unsafe {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
            );
        }
        Some(Self(handle))
    }

    /// Put a freshly spawned process into the job. Assigned after the spawn rather than before:
    /// `std::process::Command` gives no suspended start, and the services started here spawn
    /// nothing of their own in the microseconds between.
    fn adopt(&self, process: std::os::windows::io::RawHandle) {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        // SAFETY: both handles are live — ours from `create`, the child's owned by the `Child`
        // that outlives this call.
        unsafe {
            AssignProcessToJobObject(self.0, process.cast());
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: a handle this type owns, closed exactly once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(args: &[&str]) -> Args {
        parse(args.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn reads_the_log_the_env_and_the_program() {
        let args = parse_str(&[
            FLAG,
            "--log",
            r"C:\Users\adi\.adi\mono\logs\adi-app.log",
            "--env",
            "RUST_LOG=info",
            "--env",
            "ADI_DIR=.adi",
            "--",
            r"C:\Program Files\ADI\bin\adi-app.exe",
            "8000",
        ]);
        assert_eq!(
            args.log.as_deref(),
            Some(r"C:\Users\adi\.adi\mono\logs\adi-app.log")
        );
        assert_eq!(
            args.env,
            vec![
                ("RUST_LOG".to_string(), "info".to_string()),
                ("ADI_DIR".to_string(), ".adi".to_string()),
            ]
        );
        assert_eq!(
            args.program,
            vec![
                r"C:\Program Files\ADI\bin\adi-app.exe".to_string(),
                "8000".to_string()
            ]
        );
    }

    #[test]
    fn everything_after_the_separator_belongs_to_the_child() {
        // A flag the supervisor also knows is the child's once `--` has been seen, or a service
        // could never be passed one.
        let args = parse_str(&[FLAG, "--", "prog.exe", "--log", "x", "--env", "y"]);
        assert_eq!(args.log, None);
        assert!(args.env.is_empty());
        assert_eq!(args.program.len(), 5);
    }

    #[test]
    fn an_env_argument_without_a_value_is_skipped_not_fatal() {
        let args = parse_str(&[FLAG, "--env", "MALFORMED", "--", "prog.exe"]);
        assert!(args.env.is_empty());
        assert_eq!(args.program, vec!["prog.exe".to_string()]);
    }

    #[test]
    fn a_command_line_with_no_program_is_nothing_to_supervise() {
        assert!(parse_str(&[FLAG, "--log", "x"]).program.is_empty());
    }
}
