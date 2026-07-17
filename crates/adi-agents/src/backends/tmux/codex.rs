//! `tmux:codex` command construction.

use crate::AgentManifest;
use crate::arguments::TmuxCodexArguments;

pub(super) const BACKEND_ID: &str = "tmux:codex";

/// Build the Codex CLI command run by the shared tmux executor.
pub(super) fn argv(manifest: &AgentManifest<TmuxCodexArguments>) -> Vec<String> {
    let config = &manifest.arguments;
    let mut argv = vec!["codex".to_string()];
    if let Some(sandbox) = config.sandbox {
        argv.extend(["--sandbox".to_string(), sandbox.as_str().to_string()]);
    }
    if let Some(model) = &config.model {
        argv.extend(["--model".to_string(), model.clone()]);
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arguments::CodexSandbox;

    #[test]
    fn argv_honors_model_and_sandbox() {
        let manifest = AgentManifest {
            backend: BACKEND_ID.into(),
            arguments: TmuxCodexArguments {
                model: Some("gpt-5-codex".into()),
                sandbox: Some(CodexSandbox::WorkspaceWrite),
                ..TmuxCodexArguments::default()
            },
            ..AgentManifest::default()
        };
        assert_eq!(
            argv(&manifest),
            [
                "codex",
                "--sandbox",
                "workspace-write",
                "--model",
                "gpt-5-codex"
            ]
        );
    }
}
