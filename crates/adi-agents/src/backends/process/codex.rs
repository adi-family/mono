//! `process:codex` command construction (`codex exec`).

use crate::arguments::{CodexApproval, CodexSandbox, ProcessCodexArguments};
use crate::backends::push_option;

/// `workspace` is the run's resolved directory (`crate::workspace::resolve`, by way of
/// [`RunSpec::cwd`](crate::runner::RunSpec)), passed as `--cd` because it scopes Codex's sandbox,
/// not just where the process starts. The manifest's own `working_dir` already fed into that
/// resolution, so it is not read again here.
pub(crate) fn argv(
    config: &ProcessCodexArguments,
    message: &str,
    workspace: Option<&str>,
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
    if config.skip_git_repo_check {
        argv.push("--skip-git-repo-check".into());
    }
    // Always structured, never optional: `codex_stream::parse` is what turns this into steps, an
    // answer and metrics, and a plain-text run has none of those — worse, its answer is Codex's
    // own human-formatted transcript (a duplicated echo of the prompt and the reply, a token
    // count, all of it merged with the engine's own tracing), which is exactly the shape that
    // reads as corrupted prose once it reaches a reader. See `runner::detached::DetachedRunner`.
    argv.push("--json".into());
    argv.push(run_prompt(config, message));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentManifest;
    use crate::arguments::CodexReasoningEffort;

    #[test]
    fn argv_puts_global_options_before_exec_and_never_opens_a_tui() {
        let manifest = AgentManifest {
            backend: "process:codex".into(),
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
            argv(&manifest.arguments, "fix the tests", Some("/targets/acme")),
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
                "Work carefully.\n\nfix the tests",
            ]
        );
    }

    /// `--json` is not a knob: `codex_stream::parse` is the only thing that turns a run into
    /// steps, an answer and metrics, so a default-configured agent gets it same as one that asks
    /// for everything else.
    #[test]
    fn argv_always_asks_for_structured_events() {
        let argv = argv(&ProcessCodexArguments::default(), "go", None);
        assert!(argv.iter().any(|arg| arg == "--json"), "{argv:?}");
    }
}
