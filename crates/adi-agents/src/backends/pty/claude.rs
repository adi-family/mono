//! `pty:claude` command construction.

use crate::arguments::{ClaudeEffort, ClaudePermissionMode, PtyClaudeArguments};
use crate::backends::mcp::ToolScope;
use crate::backends::{push_mcp_config, push_option, push_tool_scope};

/// Build the Claude CLI command run by the shared pty executor.
pub(crate) fn argv(
    config: &PtyClaudeArguments,
    mcp: Option<&str>,
    tools: &ToolScope,
) -> Vec<String> {
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
    push_tool_scope(&mut argv, tools);
    push_option(&mut argv, "--add-dir", config.add_dir.as_deref());
    push_mcp_config(&mut argv, mcp);

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
    use crate::backends::mcp::scope_tools;

    #[test]
    fn argv_honors_model_permission_mode_and_prompt() {
        let manifest = AgentManifest {
            backend: "pty:claude".into(),
            arguments: PtyClaudeArguments {
                model: Some("opus".into()),
                permission_mode: Some(ClaudePermissionMode::Plan),
                effort: Some(ClaudeEffort::High),
                allowed_tools: Some("Read Edit".into()),
                add_dir: Some("/work".into()),
                system_prompt: Some("You are a solver.".into()),
                append_system_prompt: Some("Stay concise.".into()),
            },
            ..AgentManifest::default()
        };
        assert_eq!(
            argv(
                &manifest.arguments,
                None,
                &scope_tools(manifest.arguments.allowed_tools.as_deref()),
            ),
            [
                "claude",
                "--model",
                "opus",
                "--permission-mode",
                "plan",
                "--effort",
                "high",
                "--tools",
                "Read,Edit",
                "--allowed-tools",
                "Read,Edit,mcp__adi",
                "--add-dir",
                "/work",
                "--append-system-prompt",
                "You are a solver.\n\nStay concise.",
            ]
        );
    }
}
