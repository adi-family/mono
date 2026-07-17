//! `tmux:codex` command construction.

use crate::AgentManifest;

pub(super) const BACKEND_ID: &str = "tmux:codex";

/// Build the Codex CLI command run by the shared tmux executor.
pub(super) fn argv(manifest: &AgentManifest) -> Vec<String> {
    let mut argv = vec!["codex".to_string()];
    if let Some(sandbox) = manifest.extra.get("sandbox").filter(|s| !s.is_empty()) {
        argv.extend(["--sandbox".to_string(), sandbox.clone()]);
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
    fn argv_honors_model_and_sandbox() {
        let mut manifest = AgentManifest {
            backend: BACKEND_ID.into(),
            model: Some("gpt-5-codex".into()),
            ..AgentManifest::default()
        };
        manifest
            .extra
            .insert("sandbox".into(), "workspace-write".into());
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
