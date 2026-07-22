//! `harness:claude-sdk` command construction: the `claude` CLI run headless (`claude --print`)
//! under ADI's harness. It differs from `process:claude` by a harness turn cap (`--max-turns`) and
//! by scoping the agent to a set of adi-mono command groups.

use crate::arguments::{ClaudeEffort, ClaudePermissionMode, HarnessClaudeSdkArguments};
use crate::backends::push_option;

use super::conversation::Continuation;

pub(super) fn argv(
    config: &HarnessClaudeSdkArguments,
    message: &str,
    cont: &Continuation<'_>,
) -> Vec<String> {
    let mut argv = vec!["claude".to_string(), "--print".to_string()];
    // Stream the turn as NDJSON events (tool calls, thinking, result + metrics) so the harness can
    // show the progress of answering, not just the final text. `--verbose` is required to pair
    // `stream-json` with `--print`.
    argv.extend([
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
    ]);
    // Continuation: the first turn establishes an explicit session id; a reply resumes it, so the
    // CLI reconstructs the whole conversation. Both are turned into a single scalar flag.
    match cont {
        Continuation::First { session_id } => {
            push_option(&mut argv, "--session-id", Some(session_id));
        }
        Continuation::Resume { session_id } => {
            push_option(&mut argv, "--resume", Some(session_id));
        }
    }
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
    push_option(
        &mut argv,
        "--fallback-model",
        config.fallback_model.as_deref(),
    );
    // The harness cap on agent turns per run — the knob that distinguishes this from process:claude.
    if let Some(max_turns) = config.max_turns {
        push_option(&mut argv, "--max-turns", Some(&max_turns.to_string()));
    }

    if let Some(prompt) = append_system_prompt(config) {
        argv.extend(["--append-system-prompt".into(), prompt]);
    }
    // `--allowed-tools` / `--disallowed-tools` are variadic (`<tools...>`), so a bare positional
    // prompt right after them would be swallowed as another tool. `--` ends option parsing, so the
    // prompt is always taken as the prompt regardless of which flags precede it.
    argv.push("--".to_string());
    argv.push(run_message(message));
    argv
}

/// Fold the agent's system prompts and its adi-mono command scope into a single
/// `--append-system-prompt` value. The scope is surfaced here rather than enforced because the
/// runner-side command allow-list is future work — this at least tells the agent what it may use.
fn append_system_prompt(config: &HarnessClaudeSdkArguments) -> Option<String> {
    let mut parts = [
        config.system_prompt.as_deref(),
        config.append_system_prompt.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(ToString::to_string)
    .collect::<Vec<_>>();

    if let Some(scope) = config
        .tools
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!(
            "You may use only these adi-mono command groups: {scope}."
        ));
    }

    (!parts.is_empty()).then(|| parts.join("\n\n"))
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
    use crate::arguments::{ClaudeEffort, ClaudePermissionMode};

    #[test]
    fn argv_caps_turns_and_scopes_to_adi_commands() {
        let config = HarnessClaudeSdkArguments {
            model: Some("claude-opus-4-8".into()),
            permission_mode: Some(ClaudePermissionMode::Plan),
            effort: Some(ClaudeEffort::High),
            max_turns: Some(20),
            tools: Some("tasks,projects".into()),
            system_prompt: Some("You are a planner.".into()),
            ..HarnessClaudeSdkArguments::default()
        };
        assert_eq!(
            argv(
                &config,
                "plan the migration",
                &Continuation::First { session_id: "sid-1" }
            ),
            [
                "claude",
                "--print",
                "--output-format",
                "stream-json",
                "--verbose",
                "--session-id",
                "sid-1",
                "--model",
                "claude-opus-4-8",
                "--permission-mode",
                "plan",
                "--effort",
                "high",
                "--max-turns",
                "20",
                "--append-system-prompt",
                "You are a planner.\n\nYou may use only these adi-mono command groups: tasks,projects.",
                "--",
                "plan the migration",
            ]
        );
    }

    #[test]
    fn a_reply_resumes_the_session_instead_of_establishing_one() {
        let argv = argv(
            &HarnessClaudeSdkArguments::default(),
            "and now write a test",
            &Continuation::Resume { session_id: "sid-1" },
        );
        assert_eq!(
            argv,
            [
                "claude",
                "--print",
                "--output-format",
                "stream-json",
                "--verbose",
                "--resume",
                "sid-1",
                "--",
                "and now write a test",
            ]
        );
    }

    #[test]
    fn argv_defaults_to_a_bare_print_run() {
        let argv = argv(
            &HarnessClaudeSdkArguments::default(),
            "",
            &Continuation::First { session_id: "sid-1" },
        );
        assert_eq!(
            argv,
            [
                "claude",
                "--print",
                "--output-format",
                "stream-json",
                "--verbose",
                "--session-id",
                "sid-1",
                "--",
                "run",
            ]
        );
    }
}
