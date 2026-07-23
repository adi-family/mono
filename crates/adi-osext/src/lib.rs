//! Cross-platform OS primitives shared across the adi platform.
//!
//! macOS is the primary target, but the same binaries also build and run on Windows. A handful of
//! process- and filesystem-level operations have no portable std API, so they live here behind one
//! signature with per-OS implementations, instead of scattering `#[cfg(unix)]` / `#[cfg(windows)]`
//! blocks through every crate that spawns a child or links a file.

use std::path::Path;
use std::process::Command;

/// Whether a process with this pid is currently alive.
///
/// Shells out rather than linking libc, matching how the rest of the platform probes processes;
/// a wrong answer only degrades a status display or a stale-lock cleanup.
///
/// - **Unix:** `kill -0 <pid>` (signal 0 tests existence).
/// - **Windows:** `tasklist` filtered to the pid — it exits 0 either way, so liveness is read from
///   the output (the pid is listed only when the process is live).
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(not(unix))]
    {
        Command::new("tasklist")
            .args(["/NH", "/FI", &format!("PID eq {pid}")])
            .output()
            .is_ok_and(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
            })
    }
}

/// Detach a to-be-spawned child from the launcher's process group.
///
/// The platform launches long-lived children (agents, service daemons, hook runners) that must
/// survive the launcher and *not* receive a Ctrl-C / signal delivered to the launcher's group.
///
/// - **Unix:** `setpgid(0, 0)` via `process_group(0)` — the child leads a new group.
/// - **Windows:** `CREATE_NEW_PROCESS_GROUP` — the child is excluded from the parent's group, so a
///   console `CTRL_C_EVENT` sent to the parent's group is not delivered to it.
///
/// The `Command` is returned for chaining.
pub fn detach_process_group(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // winbase.h CREATE_NEW_PROCESS_GROUP. Kept as a literal so this crate stays dependency-free.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    cmd
}

/// Create a symbolic link at `link` pointing to the file `target`.
///
/// - **Unix:** `symlink(2)` (works for files or directories).
/// - **Windows:** `CreateSymbolicLinkW` without the directory flag. Requires the process to hold
///   `SeCreateSymbolicLinkPrivilege` (elevation, or Developer Mode enabled); otherwise it errors,
///   which callers that have a copy fallback should handle.
pub fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlinks unsupported on this platform",
        ))
    }
}

/// Create a symbolic link at `link` pointing to the directory `target`.
///
/// Same privilege caveat on Windows as [`symlink_file`], but uses the directory link flag.
pub fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlinks unsupported on this platform",
        ))
    }
}
