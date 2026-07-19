//! Project hook files (`<project>/.adi/hooks/<name>`) and their detached execution.
//!
//! A hook is a plain shell script stored git-hooks style inside the project directory, so
//! it is versionable, browsable, and editable through the project file browser. Running a
//! hook spawns `sh` detached (own process group, output to `.adi/hooks/logs/<name>.log`,
//! truncated per run) — the same execution shape as adi-triggers' fire — plus an exit-code
//! marker line appended to the log so a finished run's status is readable afterwards.

use std::fs;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

/// The lifecycle hook a project's FIRST workspace runs (e.g. `git clone`).
pub const HOOK_INIT: &str = "init";
/// The lifecycle hook every ADDITIONAL workspace runs (e.g. `git worktree add`).
pub const HOOK_WORKSPACE: &str = "workspace";
/// The marker line the run wrapper appends to the log: `[adi:hook-exit <code>]`.
pub const EXIT_MARKER_PREFIX: &str = "[adi:hook-exit ";

/// Whether `name` is one of the lifecycle hooks (`init` / `workspace`). These receive the
/// `ADI_WORKSPACE_*` env only from a workspace create, so manual-run surfaces refuse them
/// (a bare run would see an empty `$ADI_WORKSPACE_DIR` and fail confusingly).
#[must_use]
pub fn is_lifecycle(name: &str) -> bool {
    name == HOOK_INIT || name == HOOK_WORKSPACE
}

/// The hooks directory inside a project, relative to the project dir.
pub(crate) const ADI_DIR: &str = ".adi";
pub(crate) const HOOKS_DIR: &str = "hooks";
/// The per-hook run logs, under the hooks dir — excluded from hook discovery.
pub(crate) const LOGS_DIR: &str = "logs";

/// The last bytes of a run log served to callers (matches adi-triggers' tail cap).
const LOG_TAIL_MAX: u64 = 64 * 1024;

/// The shell wrapper every hook runs through. The hook body arrives via `$ADI_HOOK_CODE`
/// and runs in a *nested* `sh`, so the exit marker is appended even when the body calls
/// `exit`; the wrapper then exits with the body's code.
const WRAPPER: &str =
    r#"sh -c "$ADI_HOOK_CODE"; s=$?; printf '\n[adi:hook-exit %s]\n' "$s"; exit "$s""#;

/// The hook files of one project: discovery, creation from templates, and detached runs.
/// Constructed from the project's directory (like `adi_fs::Jail`), so the crate needs no
/// registry dependency.
#[derive(Debug, Clone)]
pub struct Hooks {
    project_dir: PathBuf,
}

/// One discovered hook file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    /// The hook's name — its file name under `.adi/hooks/`.
    pub name: String,
    /// The script's size in bytes.
    pub size: u64,
    /// The script's mtime as Unix epoch seconds.
    pub modified: Option<u64>,
}

/// A spawned hook run: the detached shell's pid and the log it writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRun {
    /// The wrapper shell's process id.
    pub pid: u32,
    /// The log file capturing the run's stdout+stderr.
    pub log: PathBuf,
}

/// What a hook's log says about its most recent run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookRunStatus {
    /// No log exists — the hook never ran.
    NeverRan,
    /// A log exists but carries no exit marker yet — the run is still going (or was cut off).
    Running,
    /// The last run finished with exit code 0.
    Ok,
    /// The last run finished with this non-zero exit code.
    Failed(i32),
}

impl HookRunStatus {
    /// The status as the wire/UI string: `never` | `running` | `ok` | `failed`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeverRan => "never",
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Failed(_) => "failed",
        }
    }

    /// The finished run's exit code (`Some(0)` for [`Self::Ok`]), or `None` while
    /// running / never ran.
    #[must_use]
    pub fn exit_code(self) -> Option<i32> {
        match self {
            Self::Ok => Some(0),
            Self::Failed(code) => Some(code),
            Self::NeverRan | Self::Running => None,
        }
    }
}

impl Hooks {
    /// Hooks over `project_dir`'s `.adi/hooks`.
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
        }
    }

    /// The project directory this instance is rooted at.
    #[must_use]
    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    /// The hooks directory: `<project>/.adi/hooks`.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.project_dir.join(ADI_DIR).join(HOOKS_DIR)
    }

    /// The hook file for `name`, after validating the name.
    ///
    /// # Errors
    /// [`Error::InvalidName`] when the name isn't a safe single segment.
    pub fn hook_path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.dir().join(name))
    }

    /// The run log for `name`: `.adi/hooks/logs/<name>.log`.
    ///
    /// # Errors
    /// [`Error::InvalidName`] when the name isn't a safe single segment.
    pub fn log_path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.dir().join(LOGS_DIR).join(format!("{name}.log")))
    }

    /// Every hook file, sorted by name. Directories (the `logs/` dir), non-UTF-8 names, and
    /// names failing validation (dotfiles like `.DS_Store`) are skipped. A missing hooks dir
    /// is an empty list.
    ///
    /// # Errors
    /// [`Error::Io`] when the directory can't be read.
    pub fn list(&self) -> Result<Vec<Hook>> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(md) = entry.metadata() else { continue };
            if md.is_dir() || validate_name(&name).is_err() {
                continue;
            }
            out.push(Hook {
                name,
                size: md.len(),
                modified: mtime_secs(&md),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Whether a hook file for `name` exists.
    #[must_use]
    pub fn exists(&self, name: &str) -> bool {
        self.hook_path(name).is_ok_and(|p| p.is_file())
    }

    /// The hook's script body.
    ///
    /// # Errors
    /// [`Error::NotFound`] when there is no such hook file, [`Error::InvalidName`] /
    /// [`Error::Io`] otherwise.
    pub fn read(&self, name: &str) -> Result<String> {
        let path = self.hook_path(name)?;
        fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound(name.to_string())
            } else {
                Error::Io(e)
            }
        })
    }

    /// Create a new hook file with `content`, refusing to overwrite an existing one (edits
    /// go through the file browser / editor, not this call).
    ///
    /// # Errors
    /// [`Error::Exists`] when the hook file is already there, [`Error::InvalidName`] /
    /// [`Error::Io`] otherwise.
    pub fn create(&self, name: &str, content: &str) -> Result<()> {
        let path = self.hook_path(name)?;
        if path.exists() {
            return Err(Error::Exists(format!("hook {name}")));
        }
        fs::create_dir_all(self.dir())?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// Run the hook detached: `sh -c` in its own process group with `PATH` augmented, the
    /// given `env` pairs plus `ADI_HOOK=<name>`, cwd at `cwd`, and stdout+stderr redirected
    /// to the hook's log (truncated — each run's log replaces the last). The wrapper appends
    /// an `[adi:hook-exit <code>]` marker line when the body finishes.
    ///
    /// # Errors
    /// [`Error::NoHook`] when the hook file is missing, [`Error::EmptyHook`] when it's
    /// blank, [`Error::Launch`] when the shell can't spawn, [`Error::InvalidName`] /
    /// [`Error::Io`] otherwise.
    pub fn run(&self, name: &str, env: &[(String, String)], cwd: &Path) -> Result<HookRun> {
        let path = self.hook_path(name)?;
        let code = fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NoHook(name.to_string())
            } else {
                Error::Io(e)
            }
        })?;
        if code.trim().is_empty() {
            return Err(Error::EmptyHook(name.to_string()));
        }

        let log = self.log_path(name)?;
        if let Some(parent) = log.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(WRAPPER)
            .env("ADI_HOOK_CODE", &code)
            .env("ADI_HOOK", name)
            .env("PATH", augmented_path())
            .process_group(0)
            .stdin(Stdio::null())
            .current_dir(cwd);
        for (key, value) in env {
            cmd.env(key, value);
        }

        let log_file = fs::File::create(&log)?;
        let errlog = log_file.try_clone()?;
        cmd.stdout(Stdio::from(log_file))
            .stderr(Stdio::from(errlog));

        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Launch(format!("couldn't spawn sh: {e}")))?;
        let pid = child.id();
        // Reap the child from a background thread: without a wait() it would linger as a
        // zombie for the spawner's lifetime, which both leaks and keeps `kill -0` (the
        // status probe) answering "alive" forever. If the spawner exits first (CLI case),
        // the run is reparented to init and reaped there — detachment is unaffected.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(HookRun { pid, log })
    }

    /// The most recent run's status, derived from the log's exit marker.
    #[must_use]
    pub fn status(&self, name: &str) -> HookRunStatus {
        let Some(log) = self.read_log(name) else {
            return HookRunStatus::NeverRan;
        };
        match parse_exit_marker(&log) {
            Some(0) => HookRunStatus::Ok,
            Some(code) => HookRunStatus::Failed(code),
            None => HookRunStatus::Running,
        }
    }

    /// When the hook last ran, as Unix epoch seconds — derived from its log's mtime (each
    /// run recreates the log). `None` if it never ran.
    #[must_use]
    pub fn last_run(&self, name: &str) -> Option<u64> {
        let path = self.log_path(name).ok()?;
        mtime_secs(&fs::metadata(path).ok()?)
    }

    /// The tail (last [`LOG_TAIL_MAX`] bytes, lossily UTF-8) of the hook's most recent run
    /// log, or `None` if it never ran. Best-effort: the run may still be appending.
    #[must_use]
    pub fn read_log(&self, name: &str) -> Option<String> {
        use std::io::Read as _;
        let path = self.log_path(name).ok()?;
        let mut file = fs::File::open(&path).ok()?;
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
}

/// The exit code from the log's final marker line, or `None` while no marker is present.
fn parse_exit_marker(log: &str) -> Option<i32> {
    let line = log.lines().rev().find(|l| !l.trim().is_empty())?;
    let rest = line.strip_prefix(EXIT_MARKER_PREFIX)?;
    rest.strip_suffix(']')?.parse().ok()
}

/// A ready-to-edit script body for the well-known hooks (`init` / `workspace`) or a `blank`
/// manual hook; `None` for an unknown template name. Each documents the env contract the
/// runner provides.
#[must_use]
pub fn hook_template(kind: &str) -> Option<&'static str> {
    match kind {
        HOOK_INIT => Some(
            r#"# adi project hook: init — creates this project's FIRST workspace.
# cwd: the parent directory of the target workspace.
# Environment:
#   ADI_PROJECT_ID / ADI_PROJECT_NAME / ADI_PROJECT_DIR
#   ADI_WORKSPACE_NAME    the new workspace's name
#   ADI_WORKSPACE_DIR     absolute target directory (this script creates it)
#   ADI_WORKSPACE_COUNT   workspaces existing before this one (0 here)
# Fail fast when run without a workspace context (use "Add workspace" instead):
[ -n "$ADI_WORKSPACE_DIR" ] || { echo "ADI_WORKSPACE_DIR is empty — this hook runs via 'Add workspace'"; exit 2; }
# Replace <REPO_URL> and edit as needed:
git clone <REPO_URL> "$ADI_WORKSPACE_DIR"
"#,
        ),
        HOOK_WORKSPACE => Some(
            r#"# adi project hook: workspace — creates each ADDITIONAL workspace.
# cwd: the primary (first) workspace directory.
# Environment: same as init, plus
#   ADI_PRIMARY_WORKSPACE_DIR   the first workspace's directory
# Fail fast when run without a workspace context (use "Add workspace" instead):
[ -n "$ADI_WORKSPACE_DIR" ] || { echo "ADI_WORKSPACE_DIR is empty — this hook runs via 'Add workspace'"; exit 2; }
# A fresh branch per workspace sidesteps git's one-checkout-per-branch rule:
git worktree add "$ADI_WORKSPACE_DIR" -b "ws-$ADI_WORKSPACE_NAME"
"#,
        ),
        "blank" => Some(
            r"# adi project hook — run it from the project's Workspaces panel or with
# `adi-mono projects hook <project> run <name>`. Output lands in .adi/hooks/logs/<name>.log.
# Environment: ADI_PROJECT_ID / ADI_PROJECT_NAME / ADI_PROJECT_DIR / ADI_HOOK.
",
        ),
        _ => None,
    }
}

/// Validate a hook/workspace name: a single, filesystem-safe path segment. This is a
/// security boundary — names arrive from the CLI and the HTTP API and are joined onto the
/// project path, so anything with a separator or `.`/`..` must be rejected. On top of the
/// project-id rule (see `adi-projects`), a leading dot is rejected (keeps `.DS_Store` and
/// friends out of discovery) and `logs` is reserved for the log directory.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name != LOGS_DIR
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_string()))
    }
}

/// Whether a process with this pid is alive, probed with `kill -0` (spares a libc/unsafe
/// dependency; wrong answers just degrade a status display).
#[must_use]
pub(crate) fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A `PATH` that includes the user's common tool directories, so a hook run under a minimal
/// launchd environment can still find `git`, `bun`, and Homebrew binaries. A verbatim copy
/// of the augmentation in adi-triggers' fire.rs and adi-webapp-api's `spawn_runner` — three
/// call sites, deliberately not worth a shared crate yet.
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

/// A metadata mtime as Unix epoch seconds.
pub(crate) fn mtime_secs(md: &fs::Metadata) -> Option<u64> {
    md.modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("adi-hooks-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Poll until `cond` holds (detached runs finish asynchronously), up to ~5s.
    pub(crate) fn wait_until(cond: impl Fn() -> bool) -> bool {
        for _ in 0..250 {
            if cond() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn valid_names_are_single_safe_segments() {
        for name in ["init", "workspace", "my-hook", "a.b", "A1_x"] {
            assert!(validate_name(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn invalid_names_are_rejected() {
        for name in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "with space",
            ".DS_Store",
            ".hidden",
            "logs",
        ] {
            assert!(
                matches!(validate_name(name), Err(Error::InvalidName(_))),
                "{name:?} should be rejected"
            );
        }
    }

    #[test]
    fn list_skips_logs_dir_and_dotfiles_and_sorts() {
        let dir = scratch_dir("list");
        let hooks = Hooks::new(&dir);
        assert!(hooks.list().unwrap().is_empty(), "missing dir lists empty");

        fs::create_dir_all(hooks.dir().join(LOGS_DIR)).unwrap();
        fs::write(hooks.dir().join("workspace"), "true").unwrap();
        fs::write(hooks.dir().join("init"), "true").unwrap();
        fs::write(hooks.dir().join(".DS_Store"), "junk").unwrap();

        let names: Vec<String> = hooks.list().unwrap().into_iter().map(|h| h.name).collect();
        assert_eq!(names, ["init", "workspace"]);
    }

    #[test]
    fn create_writes_once_and_read_round_trips() {
        let dir = scratch_dir("create");
        let hooks = Hooks::new(&dir);
        hooks.create("init", "echo hi").unwrap();
        assert!(hooks.exists("init"));
        assert_eq!(hooks.read("init").unwrap(), "echo hi");
        assert!(matches!(
            hooks.create("init", "other"),
            Err(Error::Exists(_))
        ));
        assert!(matches!(hooks.read("nope"), Err(Error::NotFound(_))));
    }

    #[test]
    fn templates_exist_for_known_kinds_only() {
        for kind in [HOOK_INIT, HOOK_WORKSPACE, "blank"] {
            assert!(hook_template(kind).is_some_and(|t| !t.is_empty()));
        }
        assert!(hook_template("nope").is_none());
    }

    #[test]
    fn run_executes_with_env_and_cwd_and_marks_exit_zero() {
        let dir = scratch_dir("run-ok");
        let hooks = Hooks::new(&dir);
        hooks
            .create("greet", "pwd; printf 'hello %s\\n' \"$ADI_TEST\"")
            .unwrap();
        let run = hooks
            .run(
                "greet",
                &[("ADI_TEST".to_string(), "world".to_string())],
                &dir,
            )
            .unwrap();
        assert!(run.pid > 0);
        assert!(wait_until(|| hooks.status("greet") == HookRunStatus::Ok));
        let log = hooks.read_log("greet").unwrap();
        assert!(log.contains("hello world"), "log: {log}");
        // macOS tempdirs live under /private; canonicalize before comparing the pwd line.
        let real = dir.canonicalize().unwrap();
        assert!(log.contains(real.to_str().unwrap()), "log: {log}");
        assert_eq!(hooks.status("greet").exit_code(), Some(0));
        assert!(hooks.last_run("greet").is_some());
    }

    #[test]
    fn run_marks_nonzero_exit_even_when_body_exits() {
        let dir = scratch_dir("run-fail");
        let hooks = Hooks::new(&dir);
        hooks.create("boom", "echo before; exit 3").unwrap();
        hooks.run("boom", &[], &dir).unwrap();
        assert!(wait_until(
            || hooks.status("boom") == HookRunStatus::Failed(3)
        ));
        assert_eq!(hooks.status("boom").exit_code(), Some(3));
    }

    #[test]
    fn run_rejects_missing_and_blank_hooks() {
        let dir = scratch_dir("run-bad");
        let hooks = Hooks::new(&dir);
        assert!(matches!(
            hooks.run("ghost", &[], &dir),
            Err(Error::NoHook(_))
        ));
        hooks.create("blank", "   \n").unwrap();
        assert!(matches!(
            hooks.run("blank", &[], &dir),
            Err(Error::EmptyHook(_))
        ));
    }

    #[test]
    fn status_is_running_while_the_body_runs() {
        let dir = scratch_dir("run-slow");
        let hooks = Hooks::new(&dir);
        assert_eq!(hooks.status("slow"), HookRunStatus::NeverRan);
        hooks.create("slow", "echo started; sleep 30").unwrap();
        let run = hooks.run("slow", &[], &dir).unwrap();
        assert!(wait_until(|| {
            hooks
                .read_log("slow")
                .is_some_and(|l| l.contains("started"))
        }));
        assert_eq!(hooks.status("slow"), HookRunStatus::Running);
        // The run is detached in its own process group (pgid = pid); reap it so the test
        // suite doesn't leave a 30s sleeper behind.
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(format!("-{}", run.pid))
            .status();
    }
}
