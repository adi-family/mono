//! `tmux:claude` command construction.

use crate::AgentManifest;

pub(super) const BACKEND_ID: &str = "tmux:claude";

/// Build the Claude CLI command run by the shared tmux executor.
pub(super) fn argv(manifest: &AgentManifest) -> Vec<String> {
    let mut argv = vec!["claude".to_string()];
    if let Some(mode) = &manifest.permission_mode {
        argv.extend(["--permission-mode".to_string(), mode.clone()]);
    }
    if !manifest.system_prompt.trim().is_empty() {
        argv.extend([
            "--append-system-prompt".to_string(),
            manifest.system_prompt.clone(),
        ]);
    }
    if let Some(model) = &manifest.model {
        argv.extend(["--model".to_string(), model.clone()]);
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_honors_model_permission_mode_and_prompt() {
        let manifest = AgentManifest {
            backend: BACKEND_ID.into(),
            model: Some("opus".into()),
            permission_mode: Some("plan".into()),
            system_prompt: "You are a solver.".into(),
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
