//! Bounded Codex app-server stdio client, embedded in the binary.

use crate::arguments::ProcessCodexArguments;

/// Whether this turn opens a Codex thread or continues one the first turn established.
///
/// Unlike Claude, Codex chooses its own thread id and reports it in the `thread.started` event.
/// The runner keeps that id after the child exits and hands it back here for every later turn.
pub(crate) enum Continuation<'a> {
    First,
    Resume { thread_id: &'a str },
}

/// `workspace` is the run's resolved directory (`crate::workspace::resolve`, by way of
/// [`RunSpec::cwd`](crate::runner::RunSpec)), passed as RPC `cwd` because it scopes Codex's sandbox,
/// not just where the process starts. The manifest's own `working_dir` already fed into that
/// resolution, so it is not read again here.
pub(crate) fn argv(
    config: &ProcessCodexArguments,
    message: &str,
    workspace: Option<&str>,
    cont: &Continuation<'_>,
) -> Vec<String> {
    let mut command = vec![
        "codex".to_string(),
        "app-server".into(),
        "--listen".into(),
        "stdio://".into(),
    ];
    if let Some(effort) = config.reasoning_effort {
        command.extend([
            "-c".into(),
            format!("model_reasoning_effort={}", effort.as_str()),
        ]);
    }
    if config.web_search {
        command.extend(["-c".into(), "web_search=\"live\"".into()]);
    }
    if let Some(dir) = &config.add_dir {
        command.extend([
            "-c".into(),
            format!(
                "sandbox_workspace_write.writable_roots=[{}]",
                serde_json::to_string(dir).unwrap()
            ),
        ]);
    }
    let mut thread = serde_json::json!({});
    if let Some(model) = &config.model {
        thread["model"] = model.clone().into();
    }
    if let Some(cwd) = workspace {
        thread["cwd"] = cwd.into();
    }
    if let Some(sandbox) = config.sandbox {
        thread["sandbox"] = sandbox.as_str().into();
    }
    if let Some(approval) = config.approval {
        thread["approvalPolicy"] = approval.as_str().into();
    }
    let message = match cont {
        Continuation::First => run_prompt(config, message),
        Continuation::Resume { thread_id } => {
            thread["threadId"] = (*thread_id).into();
            run_message(message)
        }
    };
    vec![
        "python3".into(),
        "-u".into(),
        "-c".into(),
        include_str!("codex_app_server.py").into(),
        serde_json::json!({"command": command, "thread": thread}).to_string(),
        message,
    ]
}

fn run_prompt(config: &ProcessCodexArguments, message: &str) -> String {
    let system = config.system_prompt.as_deref().unwrap_or("").trim();
    let message = message.trim();
    match (system.is_empty(), message.is_empty()) {
        (true, true) => "run".into(),
        (true, false) => message.into(),
        (false, true) => system.into(),
        (false, false) => format!("{system}\n\n{message}"),
    }
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

    #[test]
    fn rpc_parameters_preserve_options_and_literal_prompts() {
        let config = ProcessCodexArguments {
            model: Some("example".into()),
            sandbox: Some(crate::arguments::CodexSandbox::WorkspaceWrite),
            approval: Some(crate::arguments::CodexApproval::Never),
            add_dir: Some("/tmp/a space".into()),
            web_search: true,
            ..Default::default()
        };
        let args = argv(&config, "--help", Some("/workspace"), &Continuation::First);
        let opts: serde_json::Value = serde_json::from_str(&args[4]).unwrap();
        assert_eq!(opts["thread"]["cwd"], "/workspace");
        assert_eq!(opts["thread"]["model"], "example");
        assert_eq!(opts["thread"]["sandbox"], "workspace-write");
        assert_eq!(opts["thread"]["approvalPolicy"], "never");
        assert!(opts["thread"].get("threadId").is_none());
        assert_eq!(args.last().unwrap(), "--help");
        let args = argv(
            &config,
            "-",
            None,
            &Continuation::Resume { thread_id: "saved" },
        );
        let opts: serde_json::Value = serde_json::from_str(&args[4]).unwrap();
        assert_eq!(opts["thread"]["threadId"], "saved");
        assert_eq!(args.last().unwrap(), "-");
    }
}
