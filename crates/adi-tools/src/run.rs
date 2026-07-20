//! Running a tool. A tool is a small CLI an agent invokes; both ways it runs — the CLI's
//! `tools run <id>` (which inherits the caller's stdio and forwards the exit code) and the app's
//! ▶ Run button (which captures the output for the UI) — build the same [`std::process::Command`]
//! through [`command`], then decide only *how* to spawn it.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{Error, Result};
use crate::tool::{RUNTIME_TS, Tool, normalize_runtime};

/// The captured result of a one-off tool run — what the ▶ Run button shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    /// The process exit code, or `None` if it was killed by a signal.
    pub code: Option<i32>,
    /// The run's combined stdout+stderr (stderr appended after stdout).
    pub output: String,
}

impl RunOutput {
    /// Whether the run exited cleanly (`code == Some(0)`).
    #[must_use]
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Build the ready-to-spawn [`Command`] for `tool` with `args`, resolving its runtime to an
/// interpreter (`sh <script>` or `bun run <script>`), running in `working_dir`, and exporting the
/// tool's identity plus an augmented `PATH` so `bun`/Homebrew binaries resolve under a minimal
/// launchd environment.
///
/// `script_path` is where the tool's code lives — its owned `script.<ext>` in the store, or the
/// linked file on disk (the store resolves this; see [`Tools::script_path`](crate::Tools::script_path)).
///
/// # Errors
/// [`Error::LinkedMissing`] when the (linked) script file doesn't exist.
pub(crate) fn command(
    tool: &Tool,
    script_path: &Path,
    args: &[String],
    working_dir: &Path,
) -> Result<Command> {
    if !script_path.exists() {
        return Err(Error::LinkedMissing(script_path.display().to_string()));
    }

    let (program, mut argv) = match normalize_runtime(&tool.manifest.runtime) {
        RUNTIME_TS => ("bun", vec!["run".to_string(), script_path.display().to_string()]),
        // `sh` and anything a newer build might have written: run it as a shell script.
        _ => ("sh", vec![script_path.display().to_string()]),
    };
    argv.extend(args.iter().cloned());

    let mut cmd = Command::new(program);
    cmd.args(&argv)
        .current_dir(working_dir)
        .env("PATH", augmented_path())
        .env("ADI_TOOL_ID", &tool.id)
        .env("ADI_TOOL_NAME", tool.display_name());
    if let Some(project) = &tool.manifest.project {
        cmd.env("ADI_TOOL_PROJECT", project);
    }
    Ok(cmd)
}

/// Run `tool` once and capture its output — the ▶ Run path. Spawns the [`command`] with piped
/// stdio, waits for it, and returns the exit code plus combined stdout+stderr.
///
/// # Errors
/// [`Error::LinkedMissing`] when the script is gone, or [`Error::Launch`] when the interpreter
/// can't be spawned or waited on.
pub(crate) fn run_capture(
    tool: &Tool,
    script_path: &Path,
    args: &[String],
    working_dir: &Path,
) -> Result<RunOutput> {
    let mut cmd = command(tool, script_path, args, working_dir)?;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd
        .output()
        .map_err(|e| Error::Launch(format!("couldn't spawn tool: {e}")))?;

    let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.trim().is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&stderr);
    }
    Ok(RunOutput {
        code: out.status.code(),
        output,
    })
}

/// A `PATH` that includes the user's common tool directories, so a tool launched under a minimal
/// launchd environment can still find `bun`, `node`, and Homebrew binaries (the same augmentation
/// the hive runner and triggers use). `bun` living here is what makes the `ts` runtime work.
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
    use crate::tool::{Manifest, RUNTIME_SH};

    fn owned(runtime: &str) -> Tool {
        Tool {
            id: "t1".to_string(),
            manifest: Manifest {
                name: "greet".to_string(),
                runtime: runtime.to_string(),
                ..Manifest::default()
            },
        }
    }

    #[test]
    fn a_missing_script_is_refused() {
        let tool = owned(RUNTIME_SH);
        let path = std::env::temp_dir().join("adi-tools-nope-does-not-exist.sh");
        let _ = std::fs::remove_file(&path);
        assert!(matches!(
            command(&tool, &path, &[], &std::env::temp_dir()),
            Err(Error::LinkedMissing(_))
        ));
    }

    #[test]
    fn a_sh_tool_runs_its_script_and_captures_output() {
        let dir = std::env::temp_dir().join(format!("adi-tools-run-sh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let script = dir.join("script.sh");
        std::fs::write(&script, "printf '%s:%s' \"$ADI_TOOL_NAME\" \"$1\"\n").expect("write");
        let tool = owned(RUNTIME_SH);
        let out = run_capture(&tool, &script, &["hi".to_string()], &dir).expect("run");
        assert!(out.ok(), "expected clean exit, got {out:?}");
        assert_eq!(out.output, "greet:hi");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_nonzero_exit_is_reported_with_its_code() {
        let dir = std::env::temp_dir().join(format!("adi-tools-run-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let script = dir.join("script.sh");
        std::fs::write(&script, "echo boom >&2; exit 3\n").expect("write");
        let tool = owned(RUNTIME_SH);
        let out = run_capture(&tool, &script, &[], &dir).expect("run");
        assert_eq!(out.code, Some(3));
        assert!(out.output.contains("boom"), "stderr captured: {out:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_ts_tool_targets_bun() {
        let tool = owned(RUNTIME_TS);
        // The script must exist for `command` to build; content is irrelevant to the shape check.
        let dir = std::env::temp_dir().join(format!("adi-tools-run-ts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let script = dir.join("script.ts");
        std::fs::write(&script, "console.log('hi')\n").expect("write");
        let cmd = command(&tool, &script, &[], &dir).expect("command");
        assert_eq!(cmd.get_program(), "bun");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
