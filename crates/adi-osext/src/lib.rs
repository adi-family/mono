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
/// A direct syscall, deliberately: this is the platform's hottest OS probe — every run listing,
/// every conversation poll and every stale-lock sweep asks it, several times per request. Spawning
/// a helper process to answer it (`kill -0`, `tasklist`) costs a `fork` + `exec` + `wait` — about
/// 2ms each on a busy machine, all of it spent blocked in the kernel — which is how a handful of
/// pollers used to starve the app server of threads. Asking the kernel directly costs nanoseconds.
///
/// - **Unix:** `kill(pid, 0)` — signal 0 sends nothing and only runs the existence/permission
///   check. `EPERM` means the process is there but owned by someone else, so it counts as alive.
/// - **Windows:** open a `SYNCHRONIZE` handle and poll it: a live process keeps the handle
///   unsignalled (`WAIT_TIMEOUT`), an exited one signals it immediately.
///
/// Either way a wrong answer only degrades a status display or a stale-lock cleanup.
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    // Pid 0 is not a process: to `kill(2)` it means "my whole process group", which would report
    // every caller as alive. A pid file holding it is garbage, so read it as dead.
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 performs no action — `kill` only resolves the pid and runs the
        // permission check. Declared inline to keep this crate free of a `libc` dependency,
        // as `adi-hive` does for `geteuid`.
        #[allow(unsafe_code)]
        let sent = unsafe {
            unsafe extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            kill(pid, 0)
        };
        // `EPERM` is the kernel saying "it exists, but it isn't yours" — alive either way.
        sent == 0
            || std::io::Error::last_os_error().kind() == std::io::ErrorKind::PermissionDenied
    }
    #[cfg(windows)]
    {
        // kernel32 is linked by std on every Windows target, so declaring these inline keeps the
        // crate dependency-free. The constants are winnt.h `SYNCHRONIZE` and winbase.h
        // `WAIT_TIMEOUT`, kept as literals for the same reason.
        const SYNCHRONIZE: u32 = 0x0010_0000;
        const WAIT_TIMEOUT: u32 = 0x0000_0102;

        // SAFETY: the handle is null-checked before it is waited on, and closed exactly once.
        // A zero timeout makes the wait a pure poll: a live process leaves its handle
        // unsignalled, an exited one signals it at once.
        #[allow(unsafe_code)]
        unsafe {
            unsafe extern "system" {
                fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
                fn WaitForSingleObject(handle: *mut core::ffi::c_void, millis: u32) -> u32;
                fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
            }
            let handle = OpenProcess(SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                // No such process — or one we may not open at all, which reads the same as gone.
                return false;
            }
            let alive = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
            CloseHandle(handle);
            alive
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The two answers that matter: a process that is running, and one that has exited. The second
    /// is a child we reap ourselves, so its pid is known-dead rather than merely unlikely.
    #[test]
    fn liveness_follows_the_process() {
        assert!(pid_alive(std::process::id()), "we are running");

        let mut cmd = Command::new(if cfg!(windows) { "cmd" } else { "true" });
        if cfg!(windows) {
            cmd.args(["/C", "exit"]);
        }
        let mut child = cmd.spawn().expect("spawning a trivial child");
        let pid = child.id();
        child.wait().expect("reaping the child");
        assert!(!pid_alive(pid), "a reaped child is gone");
    }

    /// Pid 0 means "my process group" to `kill(2)`, which would answer yes for everyone. A pid file
    /// holding it is corrupt, and corrupt must not read as alive.
    #[test]
    fn pid_zero_is_never_alive() {
        assert!(!pid_alive(0));
    }
}
