//! `harness:claude-sdk` command construction: the `claude` CLI run headless (`claude --print`)
//! under ADI's harness. It differs from `process:claude` by a harness turn cap (`--max-turns`) and
//! by scoping the agent to a set of adi-mono command groups.

use crate::arguments::{ClaudeEffort, ClaudePermissionMode, HarnessClaudeSdkArguments};
use crate::backends::mcp::ToolScope;
use crate::backends::{push_mcp_config, push_option, push_tool_scope};
use crate::launch::expand_home;

/// Which continuation flag a turn's command carries.
///
/// Lives here, with the only engine that has such a flag, rather than in any shared vocabulary.
/// Nothing above chooses between these: the runner derives the variant from whether the session has
/// started ([`crate::runner::Session::has_started`]), so "fresh or resumed" never becomes a
/// parameter a caller can get wrong.
pub(crate) enum Continuation<'a> {
    /// The conversation's first turn — establish the session under this id.
    First { session_id: &'a str },
    /// A follow-up — resume the established session.
    Resume { session_id: &'a str },
}

pub(crate) fn argv(
    config: &HarnessClaudeSdkArguments,
    message: &str,
    cont: &Continuation<'_>,
    mcp: Option<&str>,
    tools: &ToolScope,
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
    match cont {
        Continuation::First { session_id } => {
            push_option(&mut argv, "--session-id", Some(session_id));
        }
        Continuation::Resume { session_id } => {
            push_option(&mut argv, "--resume", Some(session_id));
        }
    }
    push_option(&mut argv, "--settings", settings(config).as_deref());
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
    push_option(
        &mut argv,
        "--fallback-model",
        config.fallback_model.as_deref(),
    );
    if let Some(max_turns) = config.max_turns {
        push_option(&mut argv, "--max-turns", Some(&max_turns.to_string()));
    }

    if let Some(prompt) = append_system_prompt(config) {
        argv.extend(["--append-system-prompt".into(), prompt]);
    }
    push_mcp_config(&mut argv, mcp);
    // `--tools` / `--allowed-tools` are variadic (`<tools...>`), so a bare positional prompt right
    // after them would be swallowed as another tool. `--` ends option parsing, so the prompt is
    // always taken as the prompt regardless of which flags precede it.
    argv.push("--".to_string());
    argv.push(run_message(message));
    argv
}

/// The `--settings` value as the engine should receive it: a JSON string untouched, a path with its
/// leading `~`/`$HOME` expanded.
///
/// The expansion is the whole reason this is a function. A settings path is written by hand, in the
/// same TOML as `working_dir` and by someone who writes paths for a shell — but the run is spawned
/// directly, so a literal `~/.claude/settings.glm.json` reaches the CLI as a filename that does not
/// exist, and the agent runs on the default account while looking configured.
fn settings(config: &HarnessClaudeSdkArguments) -> Option<String> {
    let value = config
        .settings
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    if value.starts_with('{') {
        return Some(value.to_string());
    }
    Some(expand_home(value)?.display().to_string())
}

/// Fold the agent's system prompts and its adi-mono command scope into a single
/// `--append-system-prompt` value. The scope is surfaced here rather than enforced because the
/// runner-side command allow-list is future work — this at least tells the agent what it may use.
///
/// `config.tools` is the *adi-mono command* scope and has nothing to do with the engine's `--tools`
/// flag: one names groups of this platform's own commands, the other the CLI's built-in tools.
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
    use crate::backends::mcp::scope_tools;

    #[test]
    fn argv_caps_turns_and_scopes_to_adi_commands() {
        let config = HarnessClaudeSdkArguments {
            model: Some("claude-opus-4-8".into()),
            permission_mode: Some(ClaudePermissionMode::Plan),
            effort: Some(ClaudeEffort::High),
            max_turns: Some(20),
            tools: Some("tasks,projects".into()),
            system_prompt: Some("You are a planner.".into()),
            allowed_tools: Some("Read,Edit".into()),
            ..HarnessClaudeSdkArguments::default()
        };
        assert_eq!(
            argv(
                &config,
                "plan the migration",
                &Continuation::First {
                    session_id: "sid-1"
                },
                None,
                &scope_tools(config.allowed_tools.as_deref()),
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
                "--tools",
                "Read,Edit",
                "--allowed-tools",
                "Read,Edit,mcp__adi",
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
            &Continuation::Resume {
                session_id: "sid-1",
            },
            None,
            &scope_tools(None),
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
                "--tools",
                "",
                "--allowed-tools",
                "mcp__adi",
                "--",
                "and now write a test",
            ]
        );
    }

    /// Two agents, one engine, different accounts: the settings file is how a run is pointed at an
    /// `ANTHROPIC_BASE_URL` of its own. A `~` in it has to be expanded here, because the run is
    /// spawned without a shell that would have done it.
    #[test]
    fn a_settings_path_reaches_the_engine_with_its_home_expanded() {
        let home = std::env::var("HOME").expect("a test host has a HOME");
        let config = HarnessClaudeSdkArguments {
            settings: Some("~/.claude/settings.glm.json".into()),
            ..HarnessClaudeSdkArguments::default()
        };
        let argv = argv(
            &config,
            "",
            &Continuation::First {
                session_id: "sid-1",
            },
            None,
            &scope_tools(None),
        );
        let at = argv.iter().position(|a| a == "--settings").expect("--settings");
        assert_eq!(argv[at + 1], format!("{home}/.claude/settings.glm.json"));
    }

    /// The flag also takes settings inline, and JSON is not a path — expanding it would corrupt it.
    #[test]
    fn inline_settings_json_is_passed_through_untouched() {
        let json = r#"{"env":{"ANTHROPIC_BASE_URL":"https://api.z.ai/api/anthropic"}}"#;
        let config = HarnessClaudeSdkArguments {
            settings: Some(json.into()),
            ..HarnessClaudeSdkArguments::default()
        };
        let argv = argv(
            &config,
            "",
            &Continuation::First {
                session_id: "sid-1",
            },
            None,
            &scope_tools(None),
        );
        let at = argv.iter().position(|a| a == "--settings").expect("--settings");
        assert_eq!(argv[at + 1], json);
    }

    /// The bare run is the one that matters most: an agent that configured nothing still carries an
    /// explicit empty `--tools`, because the flag's *absence* is what hands over every built-in the
    /// engine ships.
    #[test]
    fn argv_defaults_to_a_bare_print_run_with_no_builtin_tools() {
        let argv = argv(
            &HarnessClaudeSdkArguments::default(),
            "",
            &Continuation::First {
                session_id: "sid-1",
            },
            None,
            &scope_tools(None),
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
                "--tools",
                "",
                "--allowed-tools",
                "mcp__adi",
                "--",
                "run",
            ]
        );
    }
}
