//! Firing a trigger — the first slice of its run layer. A fire spawns the trigger's shell code
//! block **detached** (its own process group, so it outlives the caller): the process runs in
//! the background with the trigger's identity in its environment, its output captured to
//! `triggers/logs/<name>.log`, and the event payload (a webhook body, …) handed over via
//! `ADI_PAYLOAD_FILE`. Live listeners (a Telegram poller, a cron scheduler) are future work —
//! today's sources are the app's webhook endpoint and explicit manual fires.

use std::io::Read as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};
use crate::trigger::Trigger;

/// Where a trigger's fire artifacts live under the module dir: `logs/<name>.log` (the code
/// block's combined stdout+stderr, truncated per fire) and `logs/<name>.payload` (the last
/// event payload).
const LOGS_DIR: &str = "logs";

/// A payload no larger than this is *also* exported inline as `ADI_PAYLOAD`, saving trivial
/// consumers the file read. Larger payloads are file-only — environment blocks have hard
/// platform limits a 1 MiB webhook body would blow through.
const INLINE_PAYLOAD_MAX: usize = 32 * 1024;

/// How much of the tail of a log [`read_log`] returns, so one response stays bounded no matter
/// how chatty a code block is.
const LOG_TAIL_MAX: u64 = 64 * 1024;

/// A successfully fired trigger: the spawned process and where its output lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firing {
    /// The spawned code block's process id.
    pub pid: u32,
    /// The log file capturing the run's stdout+stderr.
    pub log: PathBuf,
}

/// The log file for a trigger's fires: `<module_dir>/logs/<name>.log`.
#[must_use]
pub(crate) fn log_path(module_dir: &Path, name: &str) -> PathBuf {
    module_dir.join(LOGS_DIR).join(format!("{name}.log"))
}

/// The payload file a fire writes for its code block: `<module_dir>/logs/<name>.payload`.
#[must_use]
pub(crate) fn payload_path(module_dir: &Path, name: &str) -> PathBuf {
    module_dir.join(LOGS_DIR).join(format!("{name}.payload"))
}

/// Spawn `trigger`'s code block detached: `sh -c <code>` in its own process group, in `$HOME`
/// (a trigger shouldn't inherit the daemon's working directory), with the trigger's identity
/// and the payload in the environment, and output redirected to the trigger's log (truncated —
/// each fire's log replaces the last).
///
/// # Errors
/// [`Error::NoCode`] when the trigger has no code block, [`Error::Io`] when the log/payload
/// files can't be written, or [`Error::Launch`] when the shell can't be spawned.
pub(crate) fn fire(module_dir: &Path, trigger: &Trigger, payload: Option<&[u8]>) -> Result<Firing> {
    if trigger.manifest.code.trim().is_empty() {
        return Err(Error::NoCode(trigger.name.clone()));
    }

    let log = log_path(module_dir, &trigger.name);
    std::fs::create_dir_all(log.parent().expect("log path has the logs dir as parent"))?;

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&trigger.manifest.code)
        .env("PATH", augmented_path())
        .env("ADI_TRIGGER", &trigger.name)
        .env("ADI_TRIGGER_KIND", &trigger.manifest.kind)
        .process_group(0)
        .stdin(Stdio::null());
    if let Ok(home) = std::env::var("HOME") {
        cmd.current_dir(home);
    }

    // The payload is written *before* the spawn so the code block always finds a complete file.
    if let Some(bytes) = payload {
        let payload_file = payload_path(module_dir, &trigger.name);
        std::fs::write(&payload_file, bytes)?;
        cmd.env("ADI_PAYLOAD_FILE", &payload_file);
        if bytes.len() <= INLINE_PAYLOAD_MAX
            && let Ok(text) = std::str::from_utf8(bytes)
        {
            cmd.env("ADI_PAYLOAD", text);
        }
    }

    let log_file = std::fs::File::create(&log)?;
    let errlog = log_file.try_clone()?;
    cmd.stdout(Stdio::from(log_file)).stderr(Stdio::from(errlog));

    let child = cmd
        .spawn()
        .map_err(|e| Error::Launch(format!("couldn't spawn sh: {e}")))?;
    Ok(Firing {
        pid: child.id(),
        log,
    })
}

/// When the trigger last fired, as Unix epoch seconds — derived from its log file's mtime (each
/// fire recreates the log), so no manifest write races the fire. `None` if it never fired.
#[must_use]
pub(crate) fn last_fired(module_dir: &Path, name: &str) -> Option<u64> {
    let modified = std::fs::metadata(log_path(module_dir, name))
        .ok()?
        .modified()
        .ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// The tail (last [`LOG_TAIL_MAX`] bytes, lossily UTF-8) of the trigger's most recent fire log,
/// or `None` if it never fired. Reading is best-effort: the code block may still be running and
/// appending.
#[must_use]
pub(crate) fn read_log(module_dir: &Path, name: &str) -> Option<String> {
    let path = log_path(module_dir, name);
    let mut file = std::fs::File::open(&path).ok()?;
    let len = file.metadata().ok()?.len();
    if len > LOG_TAIL_MAX {
        use std::io::Seek as _;
        file.seek(std::io::SeekFrom::End(-LOG_TAIL_MAX.cast_signed()))
            .ok()?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// A `PATH` that includes the user's common tool directories, so a code block fired under a
/// minimal launchd environment can still find `bun`, `node`, and Homebrew binaries (the same
/// augmentation the hive runner uses).
fn augmented_path() -> String {
    let mut parts = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        parts.push(format!("{home}/.bun/bin"));
        parts.push(format!("{home}/.local/bin"));
    }
    parts.push("/opt/homebrew/bin".to_string());
    parts.push("/usr/local/bin".to_string());
    parts.push("/usr/bin".to_string());
    parts.push("/bin".to_string());
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    parts.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::TriggerManifest;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-triggers-fire-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn trigger(name: &str, code: &str) -> Trigger {
        Trigger {
            name: name.to_string(),
            manifest: TriggerManifest {
                kind: "manual".into(),
                code: code.into(),
                ..TriggerManifest::default()
            },
        }
    }

    /// Poll `path` until `pred` holds on its contents (the fired process is detached, so the
    /// log lands asynchronously).
    fn wait_for_log(path: &Path, pred: impl Fn(&str) -> bool) -> String {
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(path)
                && pred(&text)
            {
                return text;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!(
            "log at {} never matched; last contents: {:?}",
            path.display(),
            std::fs::read_to_string(path).ok()
        );
    }

    #[test]
    fn firing_without_code_is_refused() {
        let dir = scratch_dir("nocode");
        let t = trigger("empty", "   ");
        assert!(matches!(fire(&dir, &t, None), Err(Error::NoCode(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fire_runs_the_code_block_with_identity_env() {
        let dir = scratch_dir("env");
        let t = trigger("greeter", "printf '%s/%s' \"$ADI_TRIGGER\" \"$ADI_TRIGGER_KIND\"");
        let firing = fire(&dir, &t, None).expect("fire");
        assert!(firing.pid > 0);
        let text = wait_for_log(&firing.log, |s| !s.is_empty());
        assert_eq!(text, "greeter/manual");
        // The log's mtime doubles as the last-fired timestamp.
        assert!(last_fired(&dir, "greeter").is_some());
        assert_eq!(read_log(&dir, "greeter").as_deref(), Some("greeter/manual"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fire_hands_the_payload_over_as_file_and_inline_env() {
        let dir = scratch_dir("payload");
        let t = trigger("hook", "printf '%s|' \"$ADI_PAYLOAD\"; cat \"$ADI_PAYLOAD_FILE\"");
        let firing = fire(&dir, &t, Some(b"{\"x\":1}")).expect("fire");
        // The payload file is written synchronously before the spawn.
        assert_eq!(
            std::fs::read(payload_path(&dir, "hook")).expect("payload file"),
            b"{\"x\":1}"
        );
        let text = wait_for_log(&firing.log, |s| s.contains('|') && s.len() > 8);
        assert_eq!(text, "{\"x\":1}|{\"x\":1}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn never_fired_reads_as_none() {
        let dir = scratch_dir("silent");
        assert_eq!(last_fired(&dir, "ghost"), None);
        assert_eq!(read_log(&dir, "ghost"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
