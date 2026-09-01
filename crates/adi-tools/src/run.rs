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
    config: &adi_config::Config,
) -> Result<Command> {
    if !script_path.exists() {
        return Err(Error::LinkedMissing(script_path.display().to_string()));
    }

    let (program, mut argv) = match normalize_runtime(&tool.manifest.runtime) {
        RUNTIME_TS => (
            "bun",
            vec!["run".to_string(), script_path.display().to_string()],
        ),
        // `sh` and anything a newer build might have written: run it as a shell script.
        _ => ("sh", vec![script_path.display().to_string()]),
    };
    argv.extend(args.iter().cloned());

    let mut cmd = Command::new(program);
    cmd.args(&argv)
        .current_dir(working_dir)
        .env("PATH", adi_config::augmented_path())
        .env("ADI_TOOL_ID", &tool.id)
        .env("ADI_TOOL_NAME", tool.display_name());
    if let Some(project) = &tool.manifest.project {
        cmd.env("ADI_TOOL_PROJECT", project);
    }
    // Point the tool at its scope's database (a project-filed tool gets that project's), so a `ts`
    // tool's `import … from "@adi/db"` and a `sh` tool's `adi-db` both land on the same file
    // without the tool's author configuring anything.
    cmd.envs(config.db_env(tool.manifest.project.as_deref()));
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
    config: &adi_config::Config,
) -> Result<RunOutput> {
    let mut cmd = command(tool, script_path, args, working_dir, config)?;
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

    /// A store rooted in a scratch dir, so a run's `ADI_DB` points somewhere disposable.
    fn config(tag: &str) -> adi_config::Config {
        adi_config::Config::with_root(
            std::env::temp_dir().join(format!("adi-tools-run-cfg-{tag}-{}", std::process::id())),
        )
    }

    #[test]
    fn a_missing_script_is_refused() {
        let tool = owned(RUNTIME_SH);
        let path = std::env::temp_dir().join("adi-tools-nope-does-not-exist.sh");
        let _ = std::fs::remove_file(&path);
        assert!(matches!(
            command(&tool, &path, &[], &std::env::temp_dir(), &config("missing")),
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
        let out =
            run_capture(&tool, &script, &["hi".to_string()], &dir, &config("sh")).expect("run");
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
        let out = run_capture(&tool, &script, &[], &dir, &config("fail")).expect("run");
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
        let cmd = command(&tool, &script, &[], &dir, &config("ts")).expect("command");
        assert_eq!(cmd.get_program(), "bun");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_is_pointed_at_its_scope_database() {
        // What makes `adi-db` and `import … from "@adi/db"` work inside a tool without any setup:
        // the run is launched already pointed at the right file.
        let dir = std::env::temp_dir().join(format!("adi-tools-run-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let script = dir.join("script.sh");
        std::fs::write(&script, "printf '%s' \"$ADI_DB\"\n").expect("write");

        let mut tool = owned(RUNTIME_SH);
        let store = config("db");
        let global = run_capture(&tool, &script, &[], &dir, &store).expect("global run");
        assert!(
            global.output.ends_with("db/global.db"),
            "got {:?}",
            global.output
        );

        // A tool filed under a project gets that project's database instead.
        tool.manifest.project = Some("acme".to_string());
        let scoped = run_capture(&tool, &script, &[], &dir, &store).expect("project run");
        assert!(
            scoped.output.ends_with("db/projects/acme.db"),
            "got {:?}",
            scoped.output
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
