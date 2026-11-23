//! `pty:antigravity` command construction (`agy` interactive TUI).

use crate::arguments::{AntigravityMode, ClaudeEffort, PtyAntigravityArguments};
use crate::backends::push_option;

/// Build the Google Antigravity CLI (`agy`) command run by the shared pty executor.
pub(crate) fn argv(config: &PtyAntigravityArguments) -> Vec<String> {
    let mut argv = vec!["agy".to_string()];
    push_option(&mut argv, "--model", config.model.as_deref());
    push_option(
        &mut argv,
        "--effort",
        config.effort.map(ClaudeEffort::as_str),
    );
    push_option(
        &mut argv,
        "--mode",
        config.mode.map(AntigravityMode::as_str),
    );
    if config.dangerously_skip_permissions {
        argv.push("--dangerously-skip-permissions".into());
    }
    if config.terminal_sandbox {
        argv.push("--sandbox".into());
    }
    push_option(&mut argv, "--add-dir", config.add_dir.as_deref());
    if let Some(settings) = config
        .settings
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        argv.extend(["--settings".into(), expand_settings(settings)]);
    }
    push_option(&mut argv, "--agent", config.agent.as_deref());
    if let Some(prompt) = config
        .system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        argv.push(format!("--prompt-interactive={prompt}"));
    }
    argv
}

pub(crate) fn expand_settings(value: &str) -> String {
    if value.starts_with('{') {
        return value.to_string();
    }
    crate::launch::expand_home(value).map_or_else(|| value.to_string(), |p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentManifest;

    #[test]
    fn argv_honors_model_mode_and_flags() {
        let manifest = AgentManifest {
            backend: Some("pty:antigravity".into()),
            arguments: PtyAntigravityArguments {
                model: Some("gemini-4-neon-high".into()),
                effort: Some(ClaudeEffort::High),
                mode: Some(AntigravityMode::Auto),
                dangerously_skip_permissions: true,
                terminal_sandbox: true,
                add_dir: Some("/shared".into()),
                settings: Some("{\"agentMode\":\"auto\"}".into()),
                agent: Some("default".into()),
                system_prompt: Some("Fix the tests.".into()),
                ..PtyAntigravityArguments::default()
            },
            ..AgentManifest::default()
        };
        assert_eq!(
            argv(&manifest.arguments),
            [
                "agy",
                "--model",
                "gemini-4-neon-high",
                "--effort",
                "high",
                "--mode",
                "auto",
                "--dangerously-skip-permissions",
                "--sandbox",
                "--add-dir",
                "/shared",
                "--settings",
                "{\"agentMode\":\"auto\"}",
                "--agent",
                "default",
                "--prompt-interactive=Fix the tests.",
            ]
        );
    }
}
