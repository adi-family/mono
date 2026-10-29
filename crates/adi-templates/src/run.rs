//! Running a template's `command` — the `bash` engine, the one this build understands, and the
//! `requires` check that happens before it does.

use std::path::Path;

use crate::toml_file::{BASH_ENGINE, Template};

/// Where a template's command runs, and what it is told about the dashboard it is creating.
/// Every field becomes an environment variable the command reads itself — nothing here is
/// substituted into `command`'s text.
#[derive(Debug, Clone, Copy)]
pub struct Env<'a> {
    /// `cwd` for the command — the dashboards module's own root, not the dashboard's directory
    /// (which the command itself is what creates).
    pub dashboards_root: &'a Path,
    /// `$ADI_DASHBOARD_DIR` — the full path the command must create.
    pub dashboard_dir: &'a Path,
    /// `$ADI_DASHBOARD_ID`.
    pub dashboard_id: &'a str,
    /// `$ADI_DASHBOARD_HOST` — `<id>.adi`.
    pub dashboard_host: &'a str,
    /// `$ADI_TEMPLATE_DIR` — `~/.adi/mono/templates/<id>/`, for a local template's own `files/`.
    pub template_dir: &'a Path,
}

/// What a template's command printed, on either stream, when it succeeded.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub stdout: String,
    pub stderr: String,
}

/// Every way running a template is refused, or fails.
#[derive(Debug)]
pub enum RunError {
    /// `engine` named something other than [`BASH_ENGINE`] — the only one this build runs.
    UnknownEngine(String),
    /// A `requires` CLI that is not on `PATH`, checked before the command is ever started.
    MissingRequires(Vec<String>),
    /// `bash` itself could not be started.
    Spawn(std::io::Error),
    /// The command ran and exited non-zero (or was killed by a signal, `status: None`).
    Failed {
        status: Option<i32>,
        stdout: String,
        stderr: String,
    },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::UnknownEngine(engine) => write!(f, "unknown template engine {engine:?}"),
            RunError::MissingRequires(cli) => {
                write!(f, "missing required CLI(s): {}", cli.join(", "))
            }
            RunError::Spawn(e) => write!(f, "could not run bash: {e}"),
            RunError::Failed { status, stderr, .. } => {
                let code = status.map_or_else(|| "signal".to_string(), |c| c.to_string());
                write!(f, "command exited {code}: {}", stderr.trim())
            }
        }
    }
}

/// Run a template's command under its engine. The only engine today is `bash -c '<command>'`
/// ([`BASH_ENGINE`]); every `requires` CLI is checked against `PATH` first, so a missing one
/// refuses before anything runs, naming the CLI.
///
/// Leaves cleanup on failure to the caller — this crate runs the command, it does not own the
/// dashboard directory the command was asked to create.
///
/// # Errors
/// See [`RunError`].
pub fn create(template: &Template, env: Env<'_>) -> Result<RunOutcome, RunError> {
    if template.engine != BASH_ENGINE {
        return Err(RunError::UnknownEngine(template.engine.clone()));
    }
    let missing = missing_requires(template);
    if !missing.is_empty() {
        return Err(RunError::MissingRequires(missing));
    }

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(&template.command)
        .current_dir(env.dashboards_root)
        .env("ADI_DASHBOARD_DIR", env.dashboard_dir)
        .env("ADI_DASHBOARD_ID", env.dashboard_id)
        .env("ADI_DASHBOARD_HOST", env.dashboard_host)
        .env("ADI_TEMPLATE_DIR", env.template_dir)
        .output()
        .map_err(RunError::Spawn)?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        Ok(RunOutcome { stdout, stderr })
    } else {
        Err(RunError::Failed {
            status: output.status.code(),
            stdout,
            stderr,
        })
    }
}

/// Every `requires` CLI this template names that is not on `PATH`, in the order it names them.
#[must_use]
pub fn missing_requires(template: &Template) -> Vec<String> {
    template
        .requires
        .iter()
        .filter(|cli| !on_path(cli))
        .cloned()
        .collect()
}

/// Whether `cli` is an executable file somewhere on `PATH` — a minimal `which`, not a check that
/// it runs or does anything useful, since the point is only to refuse *before* the command tries
/// and fails on it instead.
#[must_use]
fn on_path(cli: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| is_executable_file(&dir.join(cli)))
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toml_file::Template;

    fn template(engine: &str, requires: Vec<&str>, command: &str) -> Template {
        Template {
            id: "t".to_string(),
            label: "T".to_string(),
            engine: engine.to_string(),
            requires: requires.into_iter().map(str::to_string).collect(),
            command: command.to_string(),
            installed_from: None,
        }
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "adi-templates-run-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    #[test]
    fn an_unknown_engine_is_refused_before_anything_runs() {
        let t = template("node", vec![], "true");
        let root = scratch("unknown-engine");
        let err = create(
            &t,
            Env {
                dashboards_root: &root,
                dashboard_dir: &root.join("d"),
                dashboard_id: "d",
                dashboard_host: "d.adi",
                template_dir: &root,
            },
        )
        .expect_err("refused");
        assert!(matches!(err, RunError::UnknownEngine(e) if e == "node"));
    }

    #[test]
    fn a_missing_required_cli_refuses_before_running_and_names_it() {
        let t = template("bash", vec!["definitely-not-a-real-cli-xyz"], "echo should never run");
        let root = scratch("missing-requires");
        let err = create(
            &t,
            Env {
                dashboards_root: &root,
                dashboard_dir: &root.join("d"),
                dashboard_id: "d",
                dashboard_host: "d.adi",
                template_dir: &root,
            },
        )
        .expect_err("refused");
        match err {
            RunError::MissingRequires(cli) => {
                assert_eq!(cli, vec!["definitely-not-a-real-cli-xyz".to_string()]);
            }
            other => panic!("expected MissingRequires, got {other:?}"),
        }
    }

    #[test]
    fn the_env_reaches_the_command_and_cwd_is_the_dashboards_root() {
        let root = scratch("env");
        let dashboard_dir = root.join("mine");
        let template_dir = root.join("templates").join("plain");
        let t = template(
            "bash",
            vec![],
            "pwd > cwd.txt && printf '%s|%s|%s|%s' \"$ADI_DASHBOARD_DIR\" \"$ADI_DASHBOARD_ID\" \"$ADI_DASHBOARD_HOST\" \"$ADI_TEMPLATE_DIR\" > env.txt",
        );
        create(
            &t,
            Env {
                dashboards_root: &root,
                dashboard_dir: &dashboard_dir,
                dashboard_id: "mine",
                dashboard_host: "mine.adi",
                template_dir: &template_dir,
            },
        )
        .expect("runs");

        let cwd = std::fs::read_to_string(root.join("cwd.txt")).expect("cwd.txt");
        assert_eq!(cwd.trim(), root.canonicalize().expect("canon").to_string_lossy());
        let env = std::fs::read_to_string(root.join("env.txt")).expect("env.txt");
        assert_eq!(
            env,
            format!(
                "{}|mine|mine.adi|{}",
                dashboard_dir.display(),
                template_dir.display()
            )
        );
    }

    #[test]
    fn a_failing_command_reports_its_output_and_exit_status() {
        let root = scratch("fail");
        let t = template("bash", vec![], "echo out-line; echo err-line 1>&2; exit 7");
        let err = create(
            &t,
            Env {
                dashboards_root: &root,
                dashboard_dir: &root.join("d"),
                dashboard_id: "d",
                dashboard_host: "d.adi",
                template_dir: &root,
            },
        )
        .expect_err("fails");
        match err {
            RunError::Failed { status, stdout, stderr } => {
                assert_eq!(status, Some(7));
                assert!(stdout.contains("out-line"), "{stdout}");
                assert!(stderr.contains("err-line"), "{stderr}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn a_succeeding_command_reports_its_output() {
        let root = scratch("ok");
        let t = template("bash", vec![], "echo hello");
        let out = create(
            &t,
            Env {
                dashboards_root: &root,
                dashboard_dir: &root.join("d"),
                dashboard_id: "d",
                dashboard_host: "d.adi",
                template_dir: &root,
            },
        )
        .expect("ok");
        assert_eq!(out.stdout.trim(), "hello");
    }
}
