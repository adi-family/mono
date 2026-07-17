//! `process:claude` command construction (`claude --print`).

use crate::arguments::{
    ClaudeEffort, ClaudeOutputFormat, ClaudePermissionMode, ProcessClaudeArguments,
};
use crate::backends::push_option;

pub(super) fn argv(config: &ProcessClaudeArguments, message: &str) -> Vec<String> {
    let mut argv = vec!["claude".to_string(), "--print".to_string()];
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
        "--output-format",
        config.output_format.map(ClaudeOutputFormat::as_str),
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
    if let Some(value) = config.max_budget_usd {
        push_option(&mut argv, "--max-budget-usd", Some(&value.to_string()));
    }
    push_option(
        &mut argv,
        "--fallback-model",
        config.fallback_model.as_deref(),
    );
    push_option(&mut argv, "--add-dir", config.add_dir.as_deref());

    let prompts = [
        config.system_prompt.as_deref(),
        config.append_system_prompt.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>();
    if !prompts.is_empty() {
        argv.extend(["--append-system-prompt".into(), prompts.join("\n\n")]);
    }
    argv.push(run_message(message));
    argv
}

fn run_message(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        "run".into()
    } else {
        message.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentManifest;

    #[test]
    fn argv_uses_print_mode_and_process_options() {
        let manifest = AgentManifest {
            backend: "process:claude".into(),
            arguments: ProcessClaudeArguments {
                model: Some("sonnet".into()),
                permission_mode: Some(ClaudePermissionMode::DontAsk),
                system_prompt: Some("You are a release agent.".into()),
                output_format: Some(ClaudeOutputFormat::Json),
                max_budget_usd: Some(2.5),
                effort: Some(ClaudeEffort::High),
                ..ProcessClaudeArguments::default()
            },
            ..AgentManifest::default()
        };
        assert_eq!(
            argv(&manifest.arguments, "prepare the release"),
            [
                "claude",
                "--print",
                "--model",
                "sonnet",
                "--permission-mode",
                "dontAsk",
                "--effort",
                "high",
                "--output-format",
                "json",
                "--max-budget-usd",
                "2.5",
                "--append-system-prompt",
                "You are a release agent.",
                "prepare the release",
            ]
        );
    }
}
