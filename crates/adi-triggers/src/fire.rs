//! Launching a trigger's code block. Both ways a trigger runs — a one-off fire (a webhook
//! delivery, a manual ▶ Fire) and a supervised background process — funnel through
//! [`Launch`], which resolves the runtime (`sh -c` or `bun run`), exports the trigger's
//! identity and settings into the environment, and stages the event payload.
//!
//! A fire spawns that launch **detached** (its own process group, so it outlives the caller)
//! with output captured to `triggers/logs/<name>.log`. The supervised path
//! ([`crate::supervisor`]) builds the same launch under tokio so it can wait on the child and
//! relaunch it.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};
use crate::trigger::{RUNTIME_TS, Trigger, normalize_runtime};

/// Where a trigger's fire artifacts live under the module dir: `logs/<name>.log` (the code
/// block's combined stdout+stderr) and `logs/<name>.payload` (the last event payload).
const LOGS_DIR: &str = "logs";

/// Where a non-shell runtime's code block is materialized so its interpreter can run it:
/// `src/<name>.ts`. Rewritten from the manifest on every launch, so it never drifts.
const SRC_DIR: &str = "src";

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

/// A resolved, ready-to-spawn invocation of a trigger's code block: what to execute, where, and
/// with which environment. Runtime-agnostic by construction — the caller only decides *how* to
/// spawn it (detached via `std::process`, or supervised via `tokio::process`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    /// The interpreter to execute (`sh`, `bun`).
    pub(crate) program: &'static str,
    /// Its arguments — the inline script for `sh -c`, or the staged module path for `bun run`.
    pub(crate) args: Vec<String>,
    /// The environment to add on top of the inherited one.
    pub(crate) env: Vec<(String, String)>,
}

/// The log file for a trigger's runs: `<module_dir>/logs/<name>.log`.
#[must_use]
pub(crate) fn log_path(module_dir: &Path, name: &str) -> PathBuf {
    module_dir.join(LOGS_DIR).join(format!("{name}.log"))
}

/// The payload file a fire writes for its code block: `<module_dir>/logs/<name>.payload`.
#[must_use]
pub(crate) fn payload_path(module_dir: &Path, name: &str) -> PathBuf {
    module_dir.join(LOGS_DIR).join(format!("{name}.payload"))
}

/// Resolve `trigger` into a spawnable [`Launch`]: pick the interpreter for its runtime (staging
/// a `ts` block to `src/<name>.ts`), export its identity and every setting, and — when the event
/// carries one — write the payload file the block reads.
///
/// # Errors
/// [`Error::NoCode`] when the trigger has no code block, or [`Error::Io`] when the staged
/// source or payload file can't be written.
pub(crate) fn launch(
    module_dir: &Path,
    trigger: &Trigger,
    payload: Option<&[u8]>,
    event: Option<&str>,
    secret_env: &[(String, String)],
) -> Result<Launch> {
    let code = trigger.manifest.code.trim();
    if code.is_empty() {
        return Err(Error::NoCode(trigger.name.clone()));
    }

    let (program, args) = match normalize_runtime(&trigger.manifest.runtime) {
        RUNTIME_TS => {
            let module = stage_source(module_dir, &trigger.name, &trigger.manifest.code, "ts")?;
            ("bun", vec!["run".to_string(), module.display().to_string()])
        }
        // `sh` and anything a newer build might have written: run it as a shell script.
        _ => ("sh", vec!["-c".to_string(), trigger.manifest.code.clone()]),
    };

    // Resolved secrets go in FIRST, under their literal key names, so the platform's own
    // reserved vars pushed below (PATH, ADI_TRIGGER*) win if a secret is unwisely named after
    // one — the injection can never break a launch by shadowing `PATH`.
    let mut env: Vec<(String, String)> = secret_env.to_vec();
    env.push(("PATH".to_string(), augmented_path()));
    env.push(("ADI_TRIGGER".to_string(), trigger.name.clone()));
    env.push((
        "ADI_TRIGGER_KIND".to_string(),
        trigger.manifest.kind.clone(),
    ));
    // For an event trigger, the concrete event name that matched — so one handler subscribed to
    // `adi.tasks.*` can branch on whether it was `created` or `updated`.
    if let Some(name) = event {
        env.push(("ADI_EVENT".to_string(), name.to_string()));
    }
    env.extend(extra_env(&trigger.manifest.extra));

    // The payload is written *before* the spawn so the code block always finds a complete file.
    if let Some(bytes) = payload {
        let file = payload_path(module_dir, &trigger.name);
        std::fs::create_dir_all(
            file.parent()
                .expect("payload path has the logs dir as parent"),
        )?;
        std::fs::write(&file, bytes)?;
        env.push(("ADI_PAYLOAD_FILE".to_string(), file.display().to_string()));
        if bytes.len() <= INLINE_PAYLOAD_MAX
            && let Ok(text) = std::str::from_utf8(bytes)
        {
            env.push(("ADI_PAYLOAD".to_string(), text.to_string()));
        }
    }

    Ok(Launch { program, args, env })
}

/// Export a trigger's settings into its code block as `ADI_<KEY>`, uppercased with `-` folded
/// to `_` — this is the contract every [preset](crate::presets) field is written against. A key
/// that isn't a usable environment-variable name is dropped rather than mangled.
fn extra_env(extra: &BTreeMap<String, String>) -> Vec<(String, String)> {
    extra
        .iter()
        .filter(|(k, _)| {
            !k.is_empty()
                && !k.starts_with(|c: char| c.is_ascii_digit())
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        })
        .map(|(k, v)| {
            (
                format!("ADI_{}", k.to_uppercase().replace('-', "_")),
                v.clone(),
            )
        })
        .collect()
}

/// Write a non-shell code block to `<module_dir>/src/<name>.<ext>` so its interpreter has a file
/// to run, returning that path. Rewritten every launch: the manifest is the source of truth.
fn stage_source(module_dir: &Path, name: &str, code: &str, ext: &str) -> Result<PathBuf> {
    let dir = module_dir.join(SRC_DIR);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{name}.{ext}"));
    std::fs::write(&file, code)?;
    Ok(file)
}

/// Open a trigger's log for writing: truncating for a one-off fire (each fire's log replaces
/// the last), appending for a supervised process (a crash loop's history is the whole point).
pub(crate) fn open_log(module_dir: &Path, name: &str, append: bool) -> Result<std::fs::File> {
    let path = log_path(module_dir, name);
    std::fs::create_dir_all(path.parent().expect("log path has the logs dir as parent"))?;
    Ok(std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(&path)?)
}

/// Spawn `trigger`'s code block detached: its own process group, in `$HOME` (a trigger shouldn't
/// inherit the daemon's working directory), with its identity and the payload in the
/// environment, and output redirected to the trigger's log (truncated — each fire replaces the
/// last).
///
/// # Errors
/// [`Error::NoCode`] when the trigger has no code block, [`Error::Io`] when the log/payload
/// files can't be written, or [`Error::Launch`] when the interpreter can't be spawned.
pub(crate) fn fire(
    module_dir: &Path,
    trigger: &Trigger,
    payload: Option<&[u8]>,
    event: Option<&str>,
    secret_env: &[(String, String)],
) -> Result<Firing> {
    let spec = launch(module_dir, trigger, payload, event, secret_env)?;
    let log_file = open_log(module_dir, &trigger.name, false)?;
    let errlog = log_file.try_clone()?;

    let mut cmd = Command::new(spec.program);
    cmd.args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(errlog));
    // One-off fire outlives the caller in its own process group / group-equivalent.
    adi_osext::detach_process_group(&mut cmd);
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.current_dir(home);
    }

    let child = cmd
        .spawn()
        .map_err(|e| Error::Launch(format!("couldn't spawn {}: {e}", spec.program)))?;
    Ok(Firing {
        pid: child.id(),
        log: log_path(module_dir, &trigger.name),
    })
}

/// When the trigger last ran, as Unix epoch seconds — derived from its log file's mtime, so no
/// manifest write races the launch. `None` if it never ran.
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

/// The tail (last [`LOG_TAIL_MAX`] bytes, lossily UTF-8) of the trigger's most recent run log,
/// or `None` if it never ran. Reading is best-effort: the code block may still be running and
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

/// A `PATH` that includes the user's common tool directories, so a code block launched under a
/// minimal launchd environment can still find `bun`, `node`, and Homebrew binaries (the same
/// augmentation the hive runner uses). `bun` living here is what makes the `ts` runtime work.
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
    use crate::trigger::{KIND_BACKGROUND, RUNTIME_SH, TriggerManifest};

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
                kind: KIND_BACKGROUND.into(),
                runtime: RUNTIME_SH.into(),
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
        assert!(matches!(
            fire(&dir, &t, None, None, &[]),
            Err(Error::NoCode(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fire_runs_the_code_block_with_identity_env() {
        let dir = scratch_dir("env");
        let t = trigger(
            "greeter",
            "printf '%s/%s' \"$ADI_TRIGGER\" \"$ADI_TRIGGER_KIND\"",
        );
        let firing = fire(&dir, &t, None, None, &[]).expect("fire");
        assert!(firing.pid > 0);
        let text = wait_for_log(&firing.log, |s| !s.is_empty());
        assert_eq!(text, "greeter/background");
        assert!(last_fired(&dir, "greeter").is_some());
        assert_eq!(
            read_log(&dir, "greeter").as_deref(),
            Some("greeter/background")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fire_hands_the_payload_over_as_file_and_inline_env() {
        let dir = scratch_dir("payload");
        let t = trigger(
            "hook",
            "printf '%s|' \"$ADI_PAYLOAD\"; cat \"$ADI_PAYLOAD_FILE\"",
        );
        let firing = fire(&dir, &t, Some(b"{\"x\":1}"), None, &[]).expect("fire");
        assert_eq!(
            std::fs::read(payload_path(&dir, "hook")).expect("payload file"),
            b"{\"x\":1}"
        );
        let text = wait_for_log(&firing.log, |s| s.contains('|') && s.len() > 8);
        assert_eq!(text, "{\"x\":1}|{\"x\":1}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An event fire hands the matched event name over as `ADI_EVENT` alongside the payload, so a
    /// handler subscribed to a wildcard can tell which concrete event it got.
    #[test]
    fn an_event_fire_exposes_the_event_name() {
        let dir = scratch_dir("eventname");
        let t = trigger("on-task", "printf '%s|%s' \"$ADI_EVENT\" \"$ADI_PAYLOAD\"");
        let firing = fire(&dir, &t, Some(b"{\"id\":\"t1\"}"), Some("adi.tasks.created"), &[])
            .expect("fire");
        assert_eq!(
            wait_for_log(&firing.log, |s| s.contains('|')),
            "adi.tasks.created|{\"id\":\"t1\"}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The preset contract: every setting reaches the code block as `ADI_<KEY>`.
    #[test]
    fn settings_reach_the_code_block_as_adi_env_vars() {
        let dir = scratch_dir("extras");
        let mut t = trigger(
            "notify",
            "printf '%s/%s' \"$ADI_CHAT_ID\" \"$ADI_TOKEN_ENV\"",
        );
        t.manifest.extra.insert("chat_id".into(), "4242".into());
        t.manifest
            .extra
            .insert("token_env".into(), "MY_TOKEN".into());
        let firing = fire(&dir, &t, None, None, &[]).expect("fire");
        assert_eq!(
            wait_for_log(&firing.log, |s| s.contains('/')),
            "4242/MY_TOKEN"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dashed_setting_key_becomes_an_underscored_env_var() {
        let extra = BTreeMap::from([
            ("chat-id".to_string(), "7".to_string()),
            ("1bad".to_string(), "x".to_string()),
            ("also bad".to_string(), "x".to_string()),
        ]);
        assert_eq!(
            extra_env(&extra),
            vec![("ADI_CHAT_ID".to_string(), "7".to_string())],
            "only the usable key survives, uppercased with dashes folded"
        );
    }

    /// Injected secrets reach the code block under their literal names, and a secret can never
    /// shadow a reserved platform var (here `ADI_TRIGGER`), which is pushed after the secrets.
    #[test]
    fn secret_env_injects_by_literal_name_and_never_shadows_platform_vars() {
        let dir = scratch_dir("secretenv");
        let t = trigger("reader", "printf '%s|%s' \"$MY_SECRET\" \"$ADI_TRIGGER\"");
        let secret_env = vec![
            ("MY_SECRET".to_string(), "s3cr3t".to_string()),
            // A secret unwisely named after a platform var must lose to the real one.
            ("ADI_TRIGGER".to_string(), "hijacked".to_string()),
        ];
        let firing = fire(&dir, &t, None, None, &secret_env).expect("fire");
        assert_eq!(
            wait_for_log(&firing.log, |s| s.contains('|')),
            "s3cr3t|reader"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `ts` trigger is staged to a real file and handed to `bun run` — the launch resolves
    /// without needing bun installed to assert the shape.
    #[test]
    fn a_typescript_block_is_staged_and_handed_to_bun() {
        let dir = scratch_dir("ts");
        let mut t = trigger("poller", "console.log('hi');\n");
        t.manifest.runtime = RUNTIME_TS.into();

        let spec = launch(&dir, &t, None, None, &[]).expect("launch");
        assert_eq!(spec.program, "bun");
        let staged = dir.join(SRC_DIR).join("poller.ts");
        assert_eq!(
            spec.args,
            vec!["run".to_string(), staged.display().to_string()]
        );
        assert_eq!(
            std::fs::read_to_string(&staged).expect("staged module"),
            "console.log('hi');\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A supervised relaunch appends, so a crash loop leaves a readable history; a one-off fire
    /// truncates, so its log is exactly that run.
    #[test]
    fn logs_append_when_supervised_and_truncate_when_fired() {
        let dir = scratch_dir("logmode");
        {
            use std::io::Write as _;
            let mut first = open_log(&dir, "svc", false).expect("open");
            write!(first, "one\n").expect("write");
            let mut second = open_log(&dir, "svc", true).expect("reopen append");
            write!(second, "two\n").expect("write");
        }
        assert_eq!(read_log(&dir, "svc").as_deref(), Some("one\ntwo\n"));

        {
            use std::io::Write as _;
            let mut fresh = open_log(&dir, "svc", false).expect("reopen truncate");
            write!(fresh, "three\n").expect("write");
        }
        assert_eq!(read_log(&dir, "svc").as_deref(), Some("three\n"));
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
