//! `pty:claude` command construction.

use crate::arguments::{ClaudeEffort, ClaudePermissionMode, PtyClaudeArguments};
use crate::backends::push_option;

/// Build the Claude CLI command run by the shared pty executor.
pub(crate) fn argv(config: &PtyClaudeArguments, mcp: Option<&str>) -> Vec<String> {
    let mut argv = vec!["claude".to_string()];
    push_option(&mut argv, "--model", config.model.as_deref());
    push_option(
        &mut argv,
        "--permission-mode",
        config.permission_mode.map(ClaudePermissionMode::as_str),
    );
    push_option(
        &mut argv,
        "--effort",
        config.effort.map(ClaudeEffort::as_str),
    );
    push_option(
        &mut argv,
        "--allowed-tools",
        config.allowed_tools.as_deref(),
    );
    push_option(
        &mut argv,
        "--disallowed-tools",
        config.disallowed_tools.as_deref(),
    );
    push_option(&mut argv, "--add-dir", config.add_dir.as_deref());
    // ADI's own tools, over MCP (see `crate::backends::mcp`). `--strict-mcp-config` rides with it so
    // the run gets *this* server and nothing else.
    if let Some(mcp) = mcp {
        argv.extend(["--mcp-config".to_string(), mcp.to_string()]);
        argv.push("--strict-mcp-config".to_string());
    }

    let prompt = [
        config.system_prompt.as_deref(),
        config.append_system_prompt.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    if !prompt.is_empty() {
        argv.extend(["--append-system-prompt".into(), prompt]);
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentManifest;
    use crate::arguments::{ClaudeEffort, ClaudePermissionMode};

    #[test]
    fn argv_honors_model_permission_mode_and_prompt() {
        let manifest = AgentManifest {
            backend: "pty:claude".into(),
            arguments: PtyClaudeArguments {
                model: Some("opus".into()),
                permission_mode: Some(ClaudePermissionMode::Plan),
                effort: Some(ClaudeEffort::High),
                allowed_tools: Some("Read Edit".into()),
                disallowed_tools: Some("WebFetch".into()),
                add_dir: Some("/work".into()),
                system_prompt: Some("You are a solver.".into()),
                append_system_prompt: Some("Stay concise.".into()),
            },
            ..AgentManifest::default()
        };
        assert_eq!(
            argv(&manifest.arguments, None),
            [
                "claude",
                "--model",
                "opus",
                "--permission-mode",
                "plan",
                "--effort",
                "high",
                "--allowed-tools",
                "Read Edit",
                "--disallowed-tools",
                "WebFetch",
                "--add-dir",
                "/work",
                "--append-system-prompt",
                "You are a solver.\n\nStay concise.",
            ]
        );
    }
}
