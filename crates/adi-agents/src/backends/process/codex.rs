//! `process:codex` command construction (`codex exec`).

use crate::arguments::{CodexApproval, CodexSandbox, ProcessCodexArguments};
use crate::backends::push_option;

pub(super) fn argv(config: &ProcessCodexArguments, message: &str) -> Vec<String> {
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
    push_option(&mut argv, "--cd", config.working_dir.as_deref());
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
    if config.json_events {
        argv.push("--json".into());
    }
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
                json_events: true,
                ..ProcessCodexArguments::default()
            },
            ..AgentManifest::default()
        };
        assert_eq!(
            argv(&manifest.arguments, "fix the tests"),
            [
                "codex",
                "--model",
                "gpt-5-codex",
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
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
}
