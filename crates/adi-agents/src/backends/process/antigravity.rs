//! `process:antigravity` command construction: bounded Antigravity ACP stdio client (`agy acp`),
//! embedded in the binary.
//!
//! Driving `agy acp` rather than one-shot `agy --print` lets every headless run:
//! - attach ADI's own `adi-mono mcp-serve` MCP server (`Bash`, `Await`, `Ask`, `PyCode`, `TsCode`)
//!   and any user-declared MCP servers per session (`session/new` / `session/load`);
//! - enforce `code_only` mode (staging a minimal `excludeDefaultComponents: true` agent profile
//!   and gating `session/request_permission` to the single interpreter tool);
//! - pass the composed system prompt natively via `_meta.antigravity.systemPrompt.append`;
//! - stream thinking chunks, tool calls, answers, and token usage in ADI's Antigravity NDJSON
//!   format (`antigravity_stream`).

use serde_json::{Value, json};

use crate::arguments::ProcessAntigravityArguments;
use crate::backends::harness::adi_loop::adi_mono_program;
use crate::backends::harness::tools::CodeTools;
use crate::backends::mcp::SERVER as ADI_MCP_SERVER;
use crate::backends::pty::antigravity::expand_settings;
use crate::backends::push_option;
use crate::mcp_servers::{ResolvedMcpServer, ResolvedTransport};
use crate::runner::{RunSpec, Session};

/// Whether this turn starts a new Antigravity conversation or continues one established by an
/// earlier turn (`session/load`).
pub(crate) enum Continuation<'a> {
    First,
    Resume { conversation_id: &'a str },
}

pub(crate) fn argv(
    config: &ProcessAntigravityArguments,
    spec: &RunSpec,
    session: &dyn Session,
    system_prompt: Option<&str>,
    message: &str,
    cont: &Continuation<'_>,
) -> Vec<String> {
    let mut command = vec!["agy".to_string()];
    if let Some(settings) = config
        .settings
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        command.extend(["--settings".into(), expand_settings(settings)]);
    }
    command.push("acp".to_string());
    if config.terminal_sandbox {
        command.push("--sandbox".into());
    }
    // Never pass `--dangerously-skip-permissions` in code-only mode: the ACP client must stay in
    // the permission loop so it can reject every tool except the single interpreter.
    if config.dangerously_skip_permissions && !spec.code.only {
        command.push("--dangerously-skip-permissions".into());
    }
    push_option(&mut command, "--add-dir", config.add_dir.as_deref());
    if !spec.code.only {
        push_option(&mut command, "--agent", config.agent.as_deref());
    }

    let cwd = spec.cwd.to_string_lossy().to_string();
    let mcp_servers = acp_mcp_servers(spec, session, &cwd);

    let mut options = json!({
        "command": command,
        "cwd": cwd,
        "mcp_servers": mcp_servers,
        "dangerously_skip_permissions": config.dangerously_skip_permissions && !spec.code.only,
    });

    if let Some(model) = config
        .model
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        options["model"] = json!(model);
    }
    if let Some(effort) = config.effort {
        options["effort"] = json!(effort.as_str());
    }
    if let Some(mode) = config.mode {
        options["mode"] = json!(mode.as_str());
    }
    if let Some(prompt) = system_prompt.map(str::trim).filter(|v| !v.is_empty()) {
        options["system_prompt"] = json!(prompt);
    }
    if let Continuation::Resume { conversation_id } = cont {
        options["conversation_id"] = json!(conversation_id);
    }
    if let Some(tool) = code_only_tool(spec.code) {
        options["code_only"] = json!(tool);
    }
    if let Some(secs) = config
        .print_timeout
        .as_deref()
        .and_then(parse_duration_seconds)
    {
        options["timeout_seconds"] = json!(secs);
    }

    vec![
        "python3".into(),
        "-u".into(),
        "-c".into(),
        include_str!("antigravity_acp.py").into(),
        options.to_string(),
        run_message(message),
    ]
}

/// Lightweight ACP argv for health/credential probes (no ADI MCP server or session attached).
pub(crate) fn probe_argv(config: &ProcessAntigravityArguments, message: &str) -> Vec<String> {
    let mut command = vec!["agy".to_string()];
    if let Some(settings) = config
        .settings
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        command.extend(["--settings".into(), expand_settings(settings)]);
    }
    command.push("acp".to_string());
    let mut options = json!({
        "command": command,
        "mcp_servers": [],
        "dangerously_skip_permissions": false,
    });
    if let Some(model) = config
        .model
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        options["model"] = json!(model);
    }
    if let Some(mode) = config.mode {
        options["mode"] = json!(mode.as_str());
    }
    vec![
        "python3".into(),
        "-u".into(),
        "-c".into(),
        include_str!("antigravity_acp.py").into(),
        options.to_string(),
        run_message(message),
    ]
}

fn code_only_tool(code: CodeTools) -> Option<&'static str> {
    if !code.only {
        return None;
    }
    if code.typescript {
        Some("TsCode")
    } else if code.python {
        Some("PyCode")
    } else {
        None
    }
}

fn acp_mcp_servers(spec: &RunSpec, session: &dyn Session, cwd: &str) -> Vec<Value> {
    let mut servers = vec![json!({
        "name": ADI_MCP_SERVER,
        "command": adi_mono_program(),
        "args": [
            "mcp-serve",
            "--agent", session.agent(),
            "--session", session.id(),
            "--dir", cwd,
        ],
        "env": [],
    })];
    // For a code-only agent, its declared MCP servers are reached from inside the interpreter via
    // ADI's `mcp-serve` hub rather than exposed directly to the engine.
    if !spec.code.only {
        for resolved in &spec.mcp_servers {
            servers.push(acp_server_json(resolved));
        }
    }
    servers
}

fn acp_server_json(resolved: &ResolvedMcpServer) -> Value {
    match &resolved.transport {
        ResolvedTransport::Stdio { command, args, env } => {
            let env_list: Vec<Value> = env
                .iter()
                .map(|(name, value)| json!({ "name": name, "value": value }))
                .collect();
            json!({
                "name": resolved.id,
                "command": command,
                "args": args,
                "env": env_list,
            })
        }
        ResolvedTransport::Http { url, headers } => {
            let header_list: Vec<Value> = headers
                .iter()
                .map(|(name, value)| json!({ "name": name, "value": value.rendered() }))
                .collect();
            json!({
                "type": "http",
                "name": resolved.id,
                "url": url,
                "headers": header_list,
            })
        }
    }
}

/// Parse a Go-style duration (`30s`, `2m`, `5m0s`, `1h30m`, or plain seconds) into positive
/// seconds. Invalid or zero durations return `None`.
fn parse_duration_seconds(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(plain) = s.parse::<f64>() {
        return (plain.is_finite() && plain > 0.0).then_some(plain);
    }
    let mut total = 0.0_f64;
    let mut rest = s;
    while !rest.is_empty() {
        let num_end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        if num_end == 0 {
            return None;
        }
        let value: f64 = rest[..num_end].parse().ok()?;
        rest = &rest[num_end..];
        let unit_end = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let unit = &rest[..unit_end];
        rest = &rest[unit_end..];
        let mult = match unit {
            "ms" => 0.001,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            _ => return None,
        };
        total += value * mult;
    }
    (total.is_finite() && total > 0.0).then_some(total)
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
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::arguments::{AntigravityMode, ClaudeEffort};
    use crate::mcp_servers::ResolvedHeader;

    struct TestSession {
        log: PathBuf,
    }

    impl Session for TestSession {
        fn agent(&self) -> &str {
            "agy-agent"
        }
        fn id(&self) -> &str {
            "sess-42"
        }
        fn log_path(&self) -> &Path {
            &self.log
        }
        fn has_started(&self) -> bool {
            false
        }
        fn state(&self) -> Option<Value> {
            None
        }
        fn set_state(&self, _state: Value) -> crate::error::Result<()> {
            Ok(())
        }
    }

    fn base_spec() -> RunSpec {
        RunSpec {
            credential: None,
            cwd: PathBuf::from("/workspace"),
            path: String::new(),
            env: Vec::new(),
            inherited_strip: Vec::new(),
            arguments: Value::Null,
            tools: Vec::new(),
            tool_help: None,
            system_prompt: None,
            workspace_note: None,
            knowledge_note: None,
            spawn_note: None,
            marker_note: None,
            mcp_servers: Vec::new(),
            code: CodeTools::default(),
        }
    }

    #[test]
    fn first_turn_builds_acp_options_with_adi_mcp_and_preserves_leading_dash_prompt() {
        let config = ProcessAntigravityArguments {
            model: Some("gemini-3.8-flash-high".into()),
            effort: Some(ClaudeEffort::Medium),
            mode: Some(AntigravityMode::Auto),
            terminal_sandbox: true,
            print_timeout: Some("2m30s".into()),
            ..Default::default()
        };
        let session = TestSession {
            log: PathBuf::from("/tmp/s.log"),
        };
        let args = argv(
            &config,
            &base_spec(),
            &session,
            Some("You are concise."),
            "--help",
            &Continuation::First,
        );
        assert_eq!(args[0], "python3");
        assert_eq!(args[1], "-u");
        assert_eq!(args[2], "-c");
        assert_eq!(args.last().map(String::as_str), Some("--help"));

        let opts: Value = serde_json::from_str(&args[4]).expect("options json");
        assert_eq!(opts["command"], json!(["agy", "acp", "--sandbox"]));
        assert_eq!(opts["cwd"], "/workspace");
        assert_eq!(opts["model"], "gemini-3.8-flash-high");
        assert_eq!(opts["effort"], "medium");
        assert_eq!(opts["mode"], "auto");
        assert_eq!(opts["system_prompt"], "You are concise.");
        assert_eq!(opts["timeout_seconds"], 150.0);
        assert!(opts.get("conversation_id").is_none());
        assert!(opts.get("code_only").is_none());

        let mcps = opts["mcp_servers"].as_array().expect("mcp_servers");
        assert_eq!(mcps.len(), 1);
        assert_eq!(mcps[0]["name"], "adi");
        assert_eq!(
            mcps[0]["args"],
            json!([
                "mcp-serve",
                "--agent",
                "agy-agent",
                "--session",
                "sess-42",
                "--dir",
                "/workspace"
            ])
        );
    }

    #[test]
    fn resume_passes_conversation_id_and_declared_mcp_servers() {
        let mut spec = base_spec();
        spec.mcp_servers = vec![
            ResolvedMcpServer {
                id: "local".into(),
                transport: ResolvedTransport::Stdio {
                    command: "npx".into(),
                    args: vec!["server".into()],
                    env: BTreeMap::from([("TOKEN".into(), "abc".into())]),
                },
            },
            ResolvedMcpServer {
                id: "remote".into(),
                transport: ResolvedTransport::Http {
                    url: "http://mcp.adi/http".into(),
                    headers: BTreeMap::from([(
                        "Authorization".into(),
                        ResolvedHeader::Prefixed {
                            prefix: "Bearer ".into(),
                            raw: "sec".into(),
                        },
                    )]),
                },
            },
        ];
        let session = TestSession {
            log: PathBuf::from("/tmp/s.log"),
        };
        let args = argv(
            &ProcessAntigravityArguments::default(),
            &spec,
            &session,
            Some("System prompt"),
            "- next step",
            &Continuation::Resume {
                conversation_id: "conv-123",
            },
        );
        let opts: Value = serde_json::from_str(&args[4]).expect("options json");
        assert_eq!(opts["conversation_id"], "conv-123");
        assert_eq!(args.last().map(String::as_str), Some("- next step"));

        let mcps = opts["mcp_servers"].as_array().expect("mcp_servers");
        assert_eq!(mcps.len(), 3);
        assert_eq!(mcps[1]["name"], "local");
        assert_eq!(mcps[1]["env"], json!([{ "name": "TOKEN", "value": "abc" }]));
        assert_eq!(mcps[2]["type"], "http");
        assert_eq!(mcps[2]["name"], "remote");
        assert_eq!(
            mcps[2]["headers"],
            json!([{ "name": "Authorization", "value": "Bearer sec" }])
        );
    }

    #[test]
    fn code_only_strips_bypass_and_user_mcp_servers_from_engine() {
        let config = ProcessAntigravityArguments {
            dangerously_skip_permissions: true,
            agent: Some("custom".into()),
            ..Default::default()
        };
        let mut spec = base_spec();
        spec.code = CodeTools::ONLY_TYPESCRIPT;
        spec.mcp_servers = vec![ResolvedMcpServer {
            id: "n8n".into(),
            transport: ResolvedTransport::Stdio {
                command: "npx".into(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
        }];
        let session = TestSession {
            log: PathBuf::from("/tmp/s.log"),
        };
        let args = argv(&config, &spec, &session, None, "go", &Continuation::First);
        let opts: Value = serde_json::from_str(&args[4]).expect("options json");
        assert_eq!(opts["code_only"], "TsCode");
        assert_eq!(opts["dangerously_skip_permissions"], false);
        assert_eq!(opts["command"], json!(["agy", "acp"]));
        let mcps = opts["mcp_servers"].as_array().expect("mcp_servers");
        assert_eq!(
            mcps.len(),
            1,
            "only ADI server goes to the engine in code-only mode"
        );
        assert_eq!(mcps[0]["name"], "adi");

        spec.code = CodeTools::ONLY_PYTHON;
        let py_args = argv(&config, &spec, &session, None, "go", &Continuation::First);
        let py_opts: Value = serde_json::from_str(&py_args[4]).expect("options json");
        assert_eq!(py_opts["code_only"], "PyCode");
    }

    #[test]
    fn parses_go_style_print_timeouts() {
        assert_eq!(parse_duration_seconds("30s"), Some(30.0));
        assert_eq!(parse_duration_seconds("5m0s"), Some(300.0));
        assert_eq!(parse_duration_seconds("1h30m"), Some(5400.0));
        assert_eq!(parse_duration_seconds("45"), Some(45.0));
        assert_eq!(parse_duration_seconds("0s"), None);
        assert_eq!(parse_duration_seconds("bogus"), None);
    }
}
