//! `process:codex` command construction (`codex exec`, then `codex exec resume`).

use crate::arguments::{CodexApproval, CodexSandbox, ProcessCodexArguments};
use crate::backends::push_option;

/// Whether this turn opens a Codex thread or continues one the first turn established.
///
/// Unlike Claude, Codex chooses its own thread id and reports it in the `thread.started` event.
/// The runner keeps that id after the child exits and hands it back here for every later turn.
pub(crate) enum Continuation<'a> {
    First,
    Resume { thread_id: &'a str },
}

/// `workspace` is the run's resolved directory (`crate::workspace::resolve`, by way of
/// [`RunSpec::cwd`](crate::runner::RunSpec)), passed as `--cd` because it scopes Codex's sandbox,
/// not just where the process starts. The manifest's own `working_dir` already fed into that
/// resolution, so it is not read again here.
pub(crate) fn argv(
    config: &ProcessCodexArguments,
    message: &str,
    workspace: Option<&str>,
    cont: &Continuation<'_>,
) -> Vec<String> {
    let mut argv = vec!["codex".to_string()];
    push_option(&mut argv, "--model", config.model.as_deref());
    push_option(
        &mut argv,
        "--sandbox",
        config.sandbox.map(CodexSandbox::as_str),
    );
    push_option(
        &mut argv,
        "--ask-for-approval",
        config.approval.map(CodexApproval::as_str),
    );
    push_option(&mut argv, "--cd", workspace);
    push_option(&mut argv, "--add-dir", config.add_dir.as_deref());
    if config.web_search {
        argv.push("--search".into());
    }
    if let Some(effort) = config.reasoning_effort {
        argv.extend([
            "--config".into(),
            format!("model_reasoning_effort={}", effort.as_str()),
        ]);
    }

    argv.push("exec".into());
    argv.extend(["--color".into(), "never".into()]);
    if matches!(cont, Continuation::Resume { .. }) {
        argv.push("resume".into());
    }
    if config.skip_git_repo_check {
        argv.push("--skip-git-repo-check".into());
    }
    // Always, because this is the format ADI reads — `crate::backends::codex_stream` is the only
    // thing that ever looks at the log, and it understands nothing else. Without it Codex writes a
    // banner, a replay of the whole prompt it was handed and a token trailer to stderr, all of
    // which shares the run's log file with the answer and used to be shown as the answer.
    argv.push("--json".into());
    match cont {
        Continuation::First => {
            // Messages are untrusted positional values. Without the separator, a prompt such as
            // `--help` is parsed as another Codex CLI flag instead of being sent to the model.
            argv.push("--".into());
            argv.push(cli_prompt(run_prompt(config, message)));
        }
        Continuation::Resume { thread_id } => {
            argv.push((*thread_id).to_string());
            argv.push("--".into());
            argv.push(cli_prompt(run_message(message)));
        }
    }
    argv
}

fn run_prompt(config: &ProcessCodexArguments, message: &str) -> String {
    let system = config.system_prompt.as_deref().unwrap_or("").trim();
    let message = message.trim();
    match (system.is_empty(), message.is_empty()) {
        (true, true) => "run".into(),
        (true, false) => message.into(),
        (false, true) => system.into(),
        (false, false) => format!("{system}\n\n{message}"),
    }
}

fn run_message(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        "run".into()
    } else {
        message.into()
    }
}

/// Keep a literal `-` a prompt rather than Codex's positional sentinel for “read stdin”.
///
/// The separator above protects leading dashes from option parsing, but `codex exec` deliberately
/// interprets the exact positional value `-` after parsing. A trailing newline makes it an ordinary
/// prompt while preserving what the model reads.
fn cli_prompt(prompt: String) -> String {
    if prompt == "-" { "-\n".into() } else { prompt }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentManifest;
    use crate::arguments::CodexReasoningEffort;

    #[test]
    fn argv_puts_global_options_before_exec_and_never_opens_a_tui() {
        let manifest = AgentManifest {
            backend: Some("process:codex".into()),
            arguments: ProcessCodexArguments {
                model: Some("gpt-5-codex".into()),
                system_prompt: Some("Work carefully.".into()),
                sandbox: Some(CodexSandbox::WorkspaceWrite),
                approval: Some(CodexApproval::Never),
                reasoning_effort: Some(CodexReasoningEffort::High),
                skip_git_repo_check: true,
                ..ProcessCodexArguments::default()
            },
            ..AgentManifest::default()
        };
        assert_eq!(
            argv(
                &manifest.arguments,
                "fix the tests",
                Some("/targets/acme"),
                &Continuation::First,
            ),
            [
                "codex",
                "--model",
                "gpt-5-codex",
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
                "--cd",
                "/targets/acme",
                "--config",
                "model_reasoning_effort=high",
                "exec",
                "--color",
                "never",
                "--skip-git-repo-check",
                "--json",
                "--",
                "Work carefully.\n\nfix the tests",
            ]
        );
    }

    /// Not a knob. `--json` is the only shape `codex_stream` can read, and an `exec` without it
    /// writes its chrome — banner, prompt replay, token trailer — into the same log as the answer.
    #[test]
    fn argv_always_asks_for_the_event_stream() {
        let argv = argv(
            &ProcessCodexArguments::default(),
            "go",
            None,
            &Continuation::First,
        );
        assert!(argv.contains(&"--json".to_string()), "{argv:?}");
    }

    #[test]
    fn a_reply_resumes_the_thread_without_repeating_the_opening_prompt() {
        let config = ProcessCodexArguments {
            system_prompt: Some("Work carefully.".into()),
            skip_git_repo_check: true,
            ..ProcessCodexArguments::default()
        };
        assert_eq!(
            argv(
                &config,
                "and now write a test",
                Some("/targets/acme"),
                &Continuation::Resume {
                    thread_id: "thread-1",
                },
            ),
            [
                "codex",
                "--cd",
                "/targets/acme",
                "exec",
                "--color",
                "never",
                "resume",
                "--skip-git-repo-check",
                "--json",
                "thread-1",
                "--",
                "and now write a test",
            ]
        );
    }

    #[test]
    fn prompt_like_a_flag_is_always_a_positional_message() {
        let first = argv(
            &ProcessCodexArguments::default(),
            "--help",
            None,
            &Continuation::First,
        );
        assert_eq!(&first[first.len() - 2..], ["--", "--help"]);

        let resumed = argv(
            &ProcessCodexArguments::default(),
            "-",
            None,
            &Continuation::Resume {
                thread_id: "thread-1",
            },
        );
        assert_eq!(&resumed[resumed.len() - 3..], ["thread-1", "--", "-\n"]);
    }
}
