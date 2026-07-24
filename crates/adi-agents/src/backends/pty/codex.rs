//! `pty:codex` command construction.

use crate::arguments::{CodexApproval, CodexSandbox, PtyCodexArguments};
use crate::backends::push_option;

/// Build the Codex CLI command run by the shared pty executor.
pub(super) fn argv(config: &PtyCodexArguments) -> Vec<String> {
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
    if let Some(prompt) = config
        .system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        argv.push(prompt.into());
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentManifest;
    use crate::arguments::{CodexReasoningEffort, CodexSandbox};

    #[test]
    fn argv_honors_model_and_sandbox() {
        let manifest = AgentManifest {
            backend: "pty:codex".into(),
            arguments: PtyCodexArguments {
                model: Some("gpt-5-codex".into()),
                sandbox: Some(CodexSandbox::WorkspaceWrite),
                approval: Some(CodexApproval::Never),
                reasoning_effort: Some(CodexReasoningEffort::High),
                working_dir: Some("/repo".into()),
                add_dir: Some("/shared".into()),
                web_search: true,
                system_prompt: Some("Fix the tests.".into()),
            },
            ..AgentManifest::default()
        };
        assert_eq!(
            argv(&manifest.arguments),
            [
                "codex",
                "--model",
                "gpt-5-codex",
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
                "--cd",
                "/repo",
                "--add-dir",
                "/shared",
                "--search",
                "--config",
                "model_reasoning_effort=high",
                "Fix the tests."
            ]
        );
    }
}
