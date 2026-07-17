//! `tmux:claude` command construction.

use crate::AgentManifest;
use crate::arguments::TmuxClaudeArguments;

pub(super) const BACKEND_ID: &str = "tmux:claude";

/// Build the Claude CLI command run by the shared tmux executor.
pub(super) fn argv(manifest: &AgentManifest<TmuxClaudeArguments>) -> Vec<String> {
    let config = &manifest.arguments;
    let mut argv = vec!["claude".to_string()];
    if let Some(mode) = config.permission_mode {
        argv.extend(["--permission-mode".to_string(), mode.as_str().to_string()]);
    }
    if let Some(system_prompt) = config
        .system_prompt
        .as_deref()
        .filter(|prompt| !prompt.trim().is_empty())
    {
        argv.extend([
            "--append-system-prompt".to_string(),
            system_prompt.to_string(),
        ]);
    }
    if let Some(model) = &config.model {
        argv.extend(["--model".to_string(), model.clone()]);
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arguments::ClaudePermissionMode;

    #[test]
    fn argv_honors_model_permission_mode_and_prompt() {
        let manifest = AgentManifest {
            backend: BACKEND_ID.into(),
            arguments: TmuxClaudeArguments {
                model: Some("opus".into()),
                permission_mode: Some(ClaudePermissionMode::Plan),
                system_prompt: Some("You are a solver.".into()),
                ..TmuxClaudeArguments::default()
            },
            ..AgentManifest::default()
        };
        assert_eq!(
            argv(&manifest),
            [
                "claude",
                "--permission-mode",
                "plan",
                "--append-system-prompt",
                "You are a solver.",
                "--model",
                "opus",
            ]
        );
    }
}
