//! Cross-platform OS primitives shared across the adi platform.
//!
//! macOS is the primary target, but the same binaries also build and run on Windows. A handful of
//! process- and filesystem-level operations have no portable std API, so they live here behind one
//! signature with per-OS implementations, instead of scattering `#[cfg(unix)]` / `#[cfg(windows)]`
//! blocks through every crate that spawns a child or links a file.

#[cfg(feature = "signals")]
mod signals;
#[cfg(feature = "signals")]
pub use signals::shutdown_signal;

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
        sent == 0 || std::io::Error::last_os_error().kind() == std::io::ErrorKind::PermissionDenied
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

/// When a process started, in unix milliseconds — the second half of a process's identity.
///
/// A pid on its own does not name a process; it names a *slot*. The kernel hands the number back
/// out once the process exits, so a pid written down on Monday can be answering for somebody else's
/// browser tab on Thursday, and [`pid_alive`] will cheerfully say yes. The pair (pid, start time)
/// is what actually identifies one incarnation: the number can be reused, but not with the same
/// start time, because the reuse happens strictly later.
///
/// Wall-clock rather than an opaque token on purpose — a recorded value has to survive a reboot in
/// a file a human may end up reading, and it is worth being able to compare it against the mtime of
/// something the process wrote.
///
/// - **macOS:** `proc_pidinfo(PROC_PIDTBSDINFO)` → `pbi_start_tvsec`/`pbi_start_tvusec`, already
///   unix time. The reply's own `pbi_pid` is checked against the pid asked about, so a struct
///   layout that ever drifts out from under this reads as "cannot tell" instead of as garbage.
/// - **Linux:** field 22 of `/proc/<pid>/stat` (start, in clock ticks since boot) resolved against
///   `btime` from `/proc/stat`. Pure file reads.
/// - **Windows:** `GetProcessTimes`, whose creation `FILETIME` is 100ns units since 1601.
///
/// `None` when the process is gone, may not be inspected, or the platform cannot say — all of which
/// callers must read as "unverifiable", never as "dead".
#[must_use]
pub fn process_start_millis(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    start_millis(pid)
}

/// [`process_start_millis`] for one platform. Split out so each implementation reads on its own
/// terms rather than as a branch of a `cfg` ladder three screens long.
#[cfg(target_os = "macos")]
fn start_millis(pid: u32) -> Option<u64> {
    // `proc_pidinfo` lives in libSystem, which every macOS binary already links, so declaring it
    // inline keeps this crate dependency-free the same way `kill` above does.
    const PROC_PIDTBSDINFO: i32 = 3;
    // `sizeof(struct proc_bsdinfo)`. The two fields this wants are the trailing pair, and
    // `pbi_pid` sits at byte 12 — read as raw bytes at fixed offsets rather than through a
    // hand-declared struct, so there is no repeat of the layout to drift.
    const PROC_BSDINFO_SIZE: usize = 136;
    const OFF_PID: usize = 12;
    const OFF_START_SEC: usize = 120;
    const OFF_START_USEC: usize = 128;

    let mut buf = [0u8; PROC_BSDINFO_SIZE];
    // SAFETY: the buffer is exactly the size handed to the call, and the reply is only read
    // back after the kernel reports it filled that many bytes.
    #[allow(unsafe_code)]
    let written = unsafe {
        unsafe extern "C" {
            fn proc_pidinfo(
                pid: i32,
                flavor: i32,
                arg: u64,
                buffer: *mut core::ffi::c_void,
                buffersize: i32,
            ) -> i32;
        }
        proc_pidinfo(
            i32::try_from(pid).ok()?,
            PROC_PIDTBSDINFO,
            0,
            buf.as_mut_ptr().cast(),
            i32::try_from(PROC_BSDINFO_SIZE).ok()?,
        )
    };
    if usize::try_from(written).ok()? != PROC_BSDINFO_SIZE {
        return None;
    }
    // The layout self-check: if this ever stops being the pid asked about, the offsets below
    // are not what this thinks they are and the honest answer is "cannot tell".
    let echoed = u32::from_ne_bytes(buf[OFF_PID..OFF_PID + 4].try_into().ok()?);
    if echoed != pid {
        return None;
    }
    let secs = u64::from_ne_bytes(buf[OFF_START_SEC..OFF_START_SEC + 8].try_into().ok()?);
    let usecs = u64::from_ne_bytes(buf[OFF_START_USEC..OFF_START_USEC + 8].try_into().ok()?);
    secs.checked_mul(1000)?.checked_add(usecs / 1000)
}

#[cfg(target_os = "linux")]
fn start_millis(pid: u32) -> Option<u64> {
    // Field 2 of `/proc/<pid>/stat` is the executable name in parentheses and may itself contain
    // spaces and parentheses, so everything is counted from the *last* `)`. After it the fields
    // resume at 3, which puts field 22 (start time) at index 19.
    const STARTTIME_AFTER_COMM: usize = 19;

    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = &stat[stat.rfind(')')? + 1..];
    let ticks: u64 = tail
        .split_whitespace()
        .nth(STARTTIME_AFTER_COMM)?
        .parse()
        .ok()?;

    // `_SC_CLK_TCK` is 2 on Linux. It is 100 on every mainstream configuration, but ask rather
    // than assume — a wrong divisor here would shift every start time by the same factor and
    // quietly make every comparison fail.
    #[allow(unsafe_code)]
    let hz = unsafe {
        unsafe extern "C" {
            fn sysconf(name: i32) -> i64;
        }
        sysconf(2)
    };
    let hz = u64::try_from(hz).unwrap_or(0).max(1);

    // Boot time, so the ticks-since-boot become a wall clock. `btime` is fixed for the life of
    // a boot, which is what makes the result stable enough to compare for equality.
    let boot_secs: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime ")?.trim().parse().ok())?;
    boot_secs
        .checked_mul(1000)?
        .checked_add(ticks.checked_mul(1000)? / hz)
}

#[cfg(windows)]
fn start_millis(pid: u32) -> Option<u64> {
    // winnt.h `PROCESS_QUERY_LIMITED_INFORMATION` — enough to read the times, and grantable for
    // processes a full query handle would be refused for.
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    // A `FILETIME` counts 100ns units from 1601-01-01; this is the gap to the unix epoch.
    const EPOCH_DIFF_MILLIS: u64 = 11_644_473_600_000;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    // SAFETY: the four out-params are owned, initialized locals; the handle is null-checked
    // before use and closed exactly once.
    #[allow(unsafe_code)]
    unsafe {
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
            fn GetProcessTimes(
                handle: *mut core::ffi::c_void,
                creation: *mut FileTime,
                exit: *mut FileTime,
                kernel: *mut FileTime,
                user: *mut FileTime,
            ) -> i32;
            fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let (mut creation, mut exit, mut kernel, mut user) = (
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
        );
        let ok = GetProcessTimes(
            handle,
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        );
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let ticks = (u64::from(creation.high) << 32) | u64::from(creation.low);
        (ticks / 10_000).checked_sub(EPOCH_DIFF_MILLIS)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn start_millis(_pid: u32) -> Option<u64> {
    None
}

/// How far two readings of the same process's start time may differ and still be the same process.
///
/// Nothing should move it at all — both readings come from [`process_start_millis`], so they are
/// normally identical to the millisecond. The slack is for the one platform that computes rather
/// than reports the value: Linux derives it from `btime`, which a clock step can nudge. A pid
/// recycled onto another process is minutes or days away from the original, never one second.
const START_SLACK_MILLIS: u64 = 2_000;

/// Whether `pid` is alive **and** is still the same process that was running when `started_millis`
/// was recorded.
///
/// This is [`pid_alive`] with the reuse hole closed. Prefer it anywhere a pid was written down and
/// read back later — a run's state slot, a pid file, a lock — and above all before *signalling*:
/// a stale pid that has been recycled belongs to an innocent process, and killing it is a great
/// deal worse than mis-reporting it.
///
/// Unverifiable reads as **not** the same process. That is the safe direction for every caller
/// here: it under-reports what is running (a finished-looking run somebody can start again) instead
/// of over-reporting it (a run nothing can clear, holding a concurrency slot, whose stop signal
/// lands on a stranger).
#[must_use]
pub fn pid_alive_as(pid: u32, started_millis: u64) -> bool {
    pid_alive(pid)
        && process_start_millis(pid)
            .is_some_and(|now| now.abs_diff(started_millis) <= START_SLACK_MILLIS)
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

/// Write a daemon's status file: create its directory, write `json`, and leave the file readable
/// by everyone.
///
/// The mode is the whole point of having this here. `adi-dns` and `adi-hive` run as root and their
/// status file is read by a per-user GUI, so a default-umask `0600` would hide from that GUI the
/// one thing it opens the file for — the port the daemon bound at runtime. Losing the `chmod` is
/// not fatal to the daemon, so it is not propagated.
///
/// Takes bytes rather than a `Serialize` value so this crate stays free of serde; the callers
/// already own their own status shape and serialize it themselves.
///
/// # Errors
/// Fails if the directory can't be created or the file can't be written — a read-only or
/// root-owned status directory. Callers log it and keep serving; the status file is a report on
/// the daemon, never a thing the daemon needs.
pub fn write_status_file(path: &Path, json: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
    }
    Ok(())
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

    /// The platform can answer the question at all, and answers it consistently. A start time that
    /// moved between two reads would fail every later comparison and report live runs as dead.
    #[test]
    fn a_process_reports_a_stable_start_time() {
        let ours = process_start_millis(std::process::id()).expect("this platform can say");
        assert!(ours > 0, "a start time is a real unix instant");
        assert_eq!(
            Some(ours),
            process_start_millis(std::process::id()),
            "asked twice, answered the same"
        );
        assert!(pid_alive_as(std::process::id(), ours));
    }

    /// The whole point: the same pid with somebody else's start time is somebody else. This is what
    /// a recycled pid looks like from the inside, without having to wait for the kernel to recycle
    /// one — the pid is real and alive, and only the start time gives it away.
    #[test]
    fn a_recycled_pid_is_not_the_process_that_was_recorded() {
        let pid = std::process::id();
        let ours = process_start_millis(pid).expect("this platform can say");

        // A day earlier: alive, right number, wrong incarnation.
        assert!(!pid_alive_as(pid, ours - 86_400_000));
        // And a day later, which is the case that actually bit — the browser tab that took the
        // number over long after the run that recorded it had finished.
        assert!(!pid_alive_as(pid, ours + 86_400_000));
    }

    /// A pid nobody is using has no start time and cannot be confirmed as anything.
    #[test]
    fn a_dead_pid_has_no_start_time() {
        let mut cmd = Command::new(if cfg!(windows) { "cmd" } else { "true" });
        if cfg!(windows) {
            cmd.args(["/C", "exit"]);
        }
        let mut child = cmd.spawn().expect("spawning a trivial child");
        let pid = child.id();
        let started = process_start_millis(pid);
        child.wait().expect("reaping the child");

        assert_eq!(
            process_start_millis(pid),
            None,
            "a reaped child has no start"
        );
        // Even holding the start time it really had while it lived, it is not alive now.
        if let Some(started) = started {
            assert!(!pid_alive_as(pid, started));
        }
    }

    /// A child's start time is its own, and lands inside the window we watched it spawn in — the
    /// property that lets a spawn record a token it can check later.
    #[test]
    fn a_child_starts_when_it_was_spawned() {
        let before = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock after 1970")
                .as_millis(),
        )
        .expect("a clock inside the next half-billion years");

        let mut cmd = Command::new(if cfg!(windows) { "cmd" } else { "sleep" });
        if cfg!(windows) {
            cmd.args(["/C", "timeout", "/T", "2"]);
        } else {
            cmd.arg("2");
        }
        let mut child = cmd.spawn().expect("spawning a sleeper");
        let pid = child.id();
        let started = process_start_millis(pid).expect("a live child has a start time");

        // Generous either way: this only has to prove the reading is of *this* spawn rather than
        // of some unrelated instant, and CI machines stall.
        assert!(
            started + 60_000 >= before,
            "child start {started} is not from before the spawn at {before}"
        );
        assert!(pid_alive_as(pid, started), "the child is itself");

        let _ = child.kill();
        let _ = child.wait();
    }

    /// Pid 0 is corrupt input here too, and must not come back with a plausible-looking answer.
    #[test]
    fn pid_zero_has_no_start_time() {
        assert_eq!(process_start_millis(0), None);
        assert!(!pid_alive_as(0, 1));
    }
}
