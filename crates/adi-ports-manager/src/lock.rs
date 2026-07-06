//! A small cross-process advisory lock guarding the registry's read-modify-write.
//!
//! Because this is a *library* linked into several processes (`adi-core`, `adi-hive`,
//! future callers), two `reserve` calls can race: both read the registry, both pick the
//! same free port, both write — one lease is lost and two services collide. A lock file
//! serializes that critical section. It is advisory (nothing forces callers through it),
//! but every mutating path in this crate takes it.
//!
//! Acquisition is `O_EXCL` create-with-retry: whoever creates the file owns the lock and
//! removes it on drop. A lock left behind by a crashed process is self-healed — if the
//! file is older than [`STALE`], it is stolen — so a crash never wedges the registry
//! permanently.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::error::{Error, Result};

/// How long to keep retrying before giving up with [`Error::LockTimeout`].
const TIMEOUT: Duration = Duration::from_secs(5);

/// Pause between acquisition attempts.
const RETRY_DELAY: Duration = Duration::from_millis(25);

/// A lock file older than this is assumed orphaned by a crashed holder and is stolen.
const STALE: Duration = Duration::from_secs(30);

/// An acquired lock. Holding the value holds the lock; dropping it releases the lock by
/// removing the file.
#[derive(Debug)]
pub struct FileLock {
    path: PathBuf,
}

impl FileLock {
    /// Acquire the lock at `path`, creating its parent directory if needed.
    ///
    /// Retries until the lock is taken or [`TIMEOUT`] elapses, stealing a lock file
    /// older than [`STALE`] on the way.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LockTimeout`] if the lock stays held past the timeout, or
    /// [`Error::Io`] on an unexpected filesystem error.
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    // Record the owner pid; purely diagnostic, failure is harmless.
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    if is_stale(path) {
                        // Best effort: another racer may remove it first, which is fine.
                        let _ = fs::remove_file(path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return Err(Error::LockTimeout {
                            path: path.to_path_buf(),
                        });
                    }
                    thread::sleep(RETRY_DELAY);
                }
                Err(e) => return Err(Error::Io(e)),
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// True if the lock file exists and its last-modified time is older than [`STALE`].
fn is_stale(path: &Path) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|m| m.modified()) else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age > STALE)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_lock_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("adi-ports-lock-{}-{tag}.lock", std::process::id()))
    }

    #[test]
    fn second_acquire_times_out_while_first_is_held() {
        let path = temp_lock_path("held");
        let _ = fs::remove_file(&path);
        let held = FileLock::acquire(&path).expect("first acquire");
        let err = FileLock::acquire(&path).expect_err("second must time out");
        assert!(matches!(err, Error::LockTimeout { .. }));
        drop(held);
    }

    #[test]
    fn releases_on_drop_so_it_can_be_retaken() {
        let path = temp_lock_path("drop");
        let _ = fs::remove_file(&path);
        {
            let _lock = FileLock::acquire(&path).expect("acquire");
        } // dropped here
        let again = FileLock::acquire(&path).expect("re-acquire after drop");
        drop(again);
        let _ = fs::remove_file(&path);
    }
}
