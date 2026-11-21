//! The MCP server that hands ADI's own tools to an engine whose loop we don't own.
//!
//! The `adi` loop calls [`tools`](super::harness::tools) directly — it *is* the loop, so a tool is
//! a function call. Every Claude engine runs its own loop and owns its own tools, and the only door
//! into it is the Model Context Protocol. This module is that door: a stdio JSON-RPC server the
//! runner registers on each Claude run, serving the *same* [`TOOLS`](super::harness::tools::TOOLS)
//! table the adi loop calls. One implementation, two ways in — which is the point. A second copy
//! written against the CLI's tool shapes would drift from the first the week after it was written.
//!
//! # What it serves, and what it deliberately does not
//!
//! Only [`SERVED`] — `Bash`, `Await` and `Ask`, and `PyCode` and `TsCode` for an agent that switched
//! them on ([`CodeTools`]). The engine's own built-in `Bash` never reaches a run
//! (see [`scope_tools`]) precisely so ours is the shell an agent gets: one shell per *conversation*,
//! where a `cd` and an `export` outlive the call and the turn (see [`super::shell`]). The CLI's own
//! `Read`/`Write`/`Edit`/`Glob`/`Grep` are better than ours would be — diff-aware edits, the CLI's
//! own permission prompts — so they are not reimplemented here; they are simply switched on for the
//! agents that ask for them.
//!
//! Our `Bash` keeps its name rather than being renamed out of the way. Measured against the CLI:
//! taking the built-in `Bash` away does not touch `mcp__adi__Bash`, because `--tools` gates the
//! built-in set alone and an MCP tool's name is server-qualified. So the vocabulary stays the one
//! [`tools`](super::harness::tools) already documents, and a prompt moves between `harness:adi` and
//! a Claude backend without learning a second set of names.
//!
//! # The transport
//!
//! MCP stdio is newline-delimited JSON-RPC 2.0 — one object per line, no `Content-Length` framing
//! (that is LSP, and confusing the two is the usual first bug). A request carries an `id` and is
//! answered; a notification carries none and must be answered with *silence*, not with a result and
//! not with an error. Anything this server writes to stdout that is not a JSON-RPC message corrupts
//! the stream, which is why nothing here prints and why a tool's own output can only ever travel
//! inside a result.
//!
//! The two specifications this file is written against, for when a method or a field below needs
//! checking against something other than this comment:
//!
//! - MCP, revision [`DEFAULT_PROTOCOL`] — <https://modelcontextprotocol.io/specification/2025-11-25>
//!   (`initialize`, `tools/list`, `tools/call`, and the stdio transport's framing rules).
//! - JSON-RPC 2.0 — <https://www.jsonrpc.org/specification> (request vs. notification, and the
//!   error codes).

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{Value, json};

use super::harness::tools::{self, CodeTools, Ctx};
use super::shell::Shell;
use crate::awaits::Awaits;
use crate::error::{Error, Result};
use crate::runner::{RunSpec, Session};
use crate::store::SessionStore;

/// The name the runner registers this server under, and therefore the `mcp__<server>__<tool>`
/// prefix the model sees. Referenced by the runner when it grants the server's tools, so the two
/// cannot disagree about the spelling.
pub(crate) const SERVER: &str = "adi";

/// The protocol version answered when a client names none. Clients send their own, and this server
/// echoes it back: it implements the parts of MCP that have not changed across these revisions —
/// initialize, list, call — so agreeing with the client is both honest and the most compatible
/// answer. Claude Code 2.1 asks for `2025-11-25`.
const DEFAULT_PROTOCOL: &str = "2025-11-25";

/// Which of the shared tools this server hands to a Claude engine. See the module docs: the file
/// tools stay the CLI's own, and only what has to be *ours* crosses the wire.
///
/// The most an agent is served, not what every agent is: the two code tools are in here and still
/// reach only an agent whose definition switched them on (see [`served_tools`]).
const SERVED: &[&str] = &["Bash", "PyCode", "TsCode", "Await", "Ask"];

/// The `--mcp-config` value that gives a Claude engine ADI's own tools: this binary, re-entered as
/// a stdio MCP server scoped to one conversation (see the module docs).
///
/// Inline JSON rather than a file, for the same reason the runner's `--settings` are inline: it is
/// *this run's*, and a file would be one more thing to place, clean up, and keep in step with a session it
/// outlives. Measured against the CLI: `--mcp-config` takes a JSON string as readily as a path,
/// though unlike `--settings` its help only advertises the path form.
///
/// The run's directory is passed explicitly instead of being inherited. This server is a
/// *grandchild* — the runner spawns the CLI, the CLI spawns the server — so the working directory it
/// starts in belongs to the CLI's process handling, and resolving an agent's relative paths against
/// it would be trusting somebody else's implementation detail.
pub(crate) fn config(spec: &RunSpec, session: &dyn Session) -> String {
    let server = serde_json::json!({
        "command": super::harness::adi_loop::adi_mono_program(),
        "args": [
            "mcp-serve",
            "--agent", session.agent(),
            "--session", session.id(),
            "--dir", spec.cwd.to_string_lossy(),
        ],
    });
    let mut servers = serde_json::Map::new();
    servers.insert(SERVER.to_string(), server);
    // The agent's own declared servers, each under its own id — resolved already (see
    // `crate::mcp_servers`), so this is only ever a reshape into the CLI's own JSON, never a
    // secret lookup. `--strict-mcp-config` (always alongside this, see `push_mcp_config`) is what
    // keeps the run from also loading whatever else is configured on this machine.
    for resolved in &spec.mcp_servers {
        servers.insert(resolved.id.clone(), server_json(resolved));
    }
    serde_json::json!({ "mcpServers": servers }).to_string()
}

/// One resolved server, in the shape `claude mcp add` itself writes — confirmed against a real
/// `claude mcp add --transport http …` / `claude mcp add -- …` run, not guessed from the schema
/// docs alone: `{"type":"http","url":…,"headers":{…}}` or `{"type":"stdio","command":…,"args":…,
/// "env":…}`. ADI's own server above omits `"type"` (stdio is the CLI's own default); a server
/// built from a definition says it anyway, since a reader comparing the two should not have to
/// know that rule to see they agree.
fn server_json(resolved: &crate::mcp_servers::ResolvedMcpServer) -> Value {
    use crate::mcp_servers::ResolvedTransport;
    match &resolved.transport {
        ResolvedTransport::Http { url, headers } => {
            // Rendered, not the resolved struct itself: a Claude engine wants the wire string
            // (`Bearer <token>`), never the prefix/raw split Codex's bearer knob alone needs.
            let headers: std::collections::BTreeMap<&String, String> =
                headers.iter().map(|(name, value)| (name, value.rendered())).collect();
            json!({ "type": "http", "url": url, "headers": headers })
        }
        ResolvedTransport::Stdio { command, args, env } => json!({
            "type": "stdio",
            "command": command,
            "args": args,
            "env": env,
        }),
    }
}

/// The engine's own shell, which no run ever gets. Ours replaces it — see the module docs for why
/// that trade is worth making, and [`super::shell`] for what the engine's own `Bash` could
/// never do.
///
/// `BashOutput` and `KillShell` go with it: they exist only to poll and kill the background jobs the
/// built-in `Bash` starts, so switching them on would advertise two tools that can no longer refer
/// to anything.
///
/// Unlike every other built-in, an agent cannot ask for these back — naming `Bash` in its own list
/// still gets it ours. It is not a preference: a second shell would be a second conversation state,
/// and the one thing this whole arrangement exists to guarantee is that there is exactly one.
pub(crate) const ENGINE_SHELL_TOOLS: &[&str] = &["Bash", "BashOutput", "KillShell"];

/// Refuse a launch outright when an agent declared MCP servers ([`crate::AgentManifest::mcp`])
/// but `backend` has no way to speak MCP to its engine at all. The field's own promise is that a
/// backend this crate does not wire up for it never just drops the servers silently — this is
/// where that promise is kept, for the two backends with no engine-side MCP door at all
/// (`harness:adi`, whose loop calls its own tools directly and never runs an engine CLI to hand
/// a config to; `pty:codex`, a live terminal with no headless config flag reachable from here).
///
/// # Errors
/// [`Error::Launch`] naming `backend` and the servers it cannot reach, when `servers` is
/// non-empty. Never refuses an agent that declared none.
pub(crate) fn refuse_unless_supported(
    backend: &crate::backend::Backend,
    servers: &[crate::mcp_servers::ResolvedMcpServer],
) -> Result<()> {
    if servers.is_empty() {
        return Ok(());
    }
    Err(Error::Launch(format!(
        "{backend} has no MCP support: it cannot be given the MCP server(s) this agent declares          ({}). Run it on harness:claude-sdk, pty:claude, process:claude, or process:codex          instead, or detach the servers with `agents save --no-mcp`",
        servers.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", "),
    )))
}

/// One run's tool surface: what exists, and what it may use without stopping to ask.
///
/// Two flags doing two different jobs, which is the confusion this type exists to end.
/// `--allowed-tools` is a *permission* list: it pre-approves calls that would otherwise prompt, and
/// a prompt is not something a headless turn can survive. It grants nothing and hides nothing —
/// everything it does not name still exists, and the model still sees it. `--tools` is the
/// *availability* list, and it is the one that denies: the run's built-in set is exactly what it
/// names, and nothing else is there to be called.
pub(crate) struct ToolScope {
    /// `--tools`: the built-ins this run has at all. Empty — the default — means none of them.
    pub(crate) builtins: String,
    /// `--allowed-tools`: those same grants, pre-approved, plus this run's own MCP server.
    pub(crate) allowed: String,
}

/// Read an agent's own tool list as the grant it is, and derive the run's whole surface from it.
///
/// **Deny by default.** A Claude run arrives holding every built-in the engine ships, and a good
/// deal of it reaches past this machine or spends money on its own: cloud cron schedules, claude.ai
/// design and trigger surfaces, push notifications, skills, subagent fan-out, a second task tracker
/// beside the platform's own. An agent definition never mentioned any of it, so defaulting it *on*
/// means every agent silently holds powers nobody granted — and the list of them grows with each
/// release of somebody else's CLI. So nothing is on unless the agent named it, and an agent that
/// names nothing runs on this run's MCP tools alone.
///
/// Measured against the CLI: with `--tools ""` and `--strict-mcp-config`, a run's advertised set is
/// exactly `mcp__adi__Bash` and `mcp__adi__Await` — `--tools` gates the built-in set only, and an
/// MCP tool's server-qualified name is never part of it. `--tools` also takes the whole list in one
/// comma-separated argument, so this returns strings rather than pushing one flag per name.
///
/// The one grant nobody has to ask for is this run's own server: an MCP tool that is registered but
/// not permitted is *advertised and then refused* — the model sees a shell, calls it, and is told it
/// lacks permission — so the server-level grant (`mcp__adi`) always rides along. It covers every
/// tool the server serves, so serving one more later needs no change here.
///
/// A scoped rule keeps its scope where it is a rule and loses it where it is a name: `Edit(src/**)`
/// is pre-approved exactly as written, and switches on the `Edit` tool.
pub(crate) fn scope_tools(allowed: Option<&str>) -> ToolScope {
    let mut builtins: Vec<String> = Vec::new();
    let mut grants: Vec<String> = Vec::new();
    for entry in entries(allowed) {
        let name = tool_name(&entry);
        if ENGINE_SHELL_TOOLS.contains(&name) {
            continue;
        }
        if !grants.iter().any(|granted| granted == &entry) {
            grants.push(entry.clone());
        }
        // An MCP tool is not part of the built-in set `--tools` gates, and naming one there is not
        // a request the CLI understands.
        if !name.starts_with("mcp__") && !builtins.iter().any(|listed| listed == name) {
            builtins.push(name.to_string());
        }
    }
    let server = format!("mcp__{SERVER}");
    if !grants.iter().any(|granted| granted == &server) {
        grants.push(server);
    }
    ToolScope {
        builtins: builtins.join(","),
        allowed: grants.join(","),
    }
}

/// Extend a run's tool scope with its own declared MCP servers, each granted whole
/// (`mcp__<id>`) — the same unconditional, ungated grant [`scope_tools`] already gives this run's
/// own server. Separate from `scope_tools` rather than folded into it, so every test exercising
/// the built-in scope alone (by far most of them) is untouched by an agent that also happens to
/// declare MCP servers; a caller that launches an engine actually reaches this.
///
/// Granted whole rather than per-tool, on the same reasoning `mcp__adi` already rests on: naming
/// a server here is naming it as a whole capability, and `tools/list` is how the model learns
/// what it can call on it — a second, finer-grained allowlist of individual `mcp__<id>__<tool>`
/// names would be a thing nothing here can derive without first asking the server.
pub(crate) fn grant_servers(scope: &mut ToolScope, servers: &[crate::mcp_servers::ResolvedMcpServer]) {
    for resolved in servers {
        let grant = format!("mcp__{}", resolved.id);
        if scope.allowed.split(',').any(|g| g == grant) {
            continue;
        }
        if scope.allowed.is_empty() {
            scope.allowed = grant;
        } else {
            scope.allowed.push(',');
            scope.allowed.push_str(&grant);
        }
    }
}

/// Split a tool list into entries, in either spelling that reaches us — the CLI takes comma- or
/// space-separated lists, and both are written in the wild — without cutting a rule in half.
/// `Bash(git *)` holds a space of its own, so a plain `split_whitespace` turns one rule into two
/// entries that name nothing.
fn entries(list: Option<&str>) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    let mut depth = 0_usize;
    for ch in list.unwrap_or_default().chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => take(&mut found, &mut current),
            ch if ch.is_whitespace() && depth == 0 => take(&mut found, &mut current),
            ch => current.push(ch),
        }
    }
    take(&mut found, &mut current);
    found
}

/// Close off the entry being read, keeping it only if it is not empty — a list may be written with
/// stray separators, and `Read,,Edit` names two tools rather than three.
fn take(found: &mut Vec<String>, current: &mut String) {
    let entry = current.trim();
    if !entry.is_empty() {
        found.push(entry.to_string());
    }
    current.clear();
}

/// The tool a list entry names, with any rule scope dropped: `Edit(src/**)` is the `Edit` tool.
fn tool_name(entry: &str) -> &str {
    entry.split('(').next().unwrap_or(entry).trim()
}

/// What an agent's definition says about the tools this server hands out: the two things that
/// are the agent's to decide rather than the conversation's.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Standing {
    /// Nobody is watching this agent, so `Ask` refuses.
    pub(crate) unattended: bool,
    /// Which code tools it switched on.
    pub(crate) code: CodeTools,
}

/// Serve one run's tools until the client closes the pipe.
///
/// `cwd` is the run's own directory, passed in rather than read from the process: this server is a
/// grandchild — the runner spawns the engine's CLI, and the CLI spawns us — so the working
/// directory we inherit is the CLI's business and not a thing to build path resolution on.
///
/// # Errors
/// [`Error::Process`] if the transport itself fails. A tool that fails is *not* an error here: its
/// message is the answer the model reads, exactly as in the adi loop.
pub(crate) fn serve(
    agent: &str,
    conv: &str,
    cwd: &Path,
    sessions: &SessionStore,
    standing: Standing,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<()> {
    let agent_dir = sessions.agent_dir(agent);
    // The conversation's shell keeps its state in sidecars of this directory and writes them from
    // inside the command it runs, so a missing directory is not an error anybody sees — it is a
    // `Bash` reporting a redirection failure instead of the output the model asked for. Made here
    // because this process is a grandchild of the runner and cannot assume any write path ran first.
    std::fs::create_dir_all(&agent_dir)?;
    let ctx = Ctx {
        cwd,
        // The conversation's shell, keyed by session id exactly as the adi loop keys it — so one
        // conversation has one shell whichever engine answered which turn.
        shell: Shell::new(&agent_dir, conv),
        agent,
        conv,
        awaits: Awaits::open(),
        sessions: sessions.clone(),
        agent_dir: &agent_dir,
        unattended: standing.unattended,
        code: standing.code,
    };

    for line in input.lines() {
        let line = line.map_err(|e| Error::Process(format!("mcp: couldn't read stdin: {e}")))?;
        if line.trim().is_empty() {
            continue;
        }
        // A line we can't parse has no id to answer under, so there is nothing to reply to and
        // nothing to gain by dying: skip it and keep serving.
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(reply) = answer(&request, &ctx) {
            writeln!(output, "{reply}")
                .and_then(|()| output.flush())
                .map_err(|e| Error::Process(format!("mcp: couldn't write stdout: {e}")))?;
        }
    }
    Ok(())
}

/// The reply one message deserves, or `None` for a notification — which is every message without an
/// `id`. Answering one is a protocol violation, and a client that gets a result for the
/// `notifications/initialized` it just sent is entitled to close the connection.
fn answer(request: &Value, ctx: &Ctx<'_>) -> Option<String> {
    let id = request.get("id").filter(|id| !id.is_null())?.clone();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");

    let result = match method {
        "initialize" => Ok(initialize(request)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": served_tools(ctx.code) })),
        "tools/call" => call(request, ctx),
        other => Err(format!("no method {other}")),
    };

    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        // -32601 is JSON-RPC's "method not found" (<https://www.jsonrpc.org/specification#error_object>),
        // the only protocol-level error this server can raise: a *tool* that fails is a successful
        // call carrying `isError`, not a failed request.
        Err(message) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": message },
        })
        .to_string(),
    })
}

/// The handshake. Capabilities are honest about what is here: tools, and nothing else — no
/// resources, no prompts, no sampling.
fn initialize(request: &Value) -> Value {
    let protocol = request
        .pointer("/params/protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL);
    json!({
        "protocolVersion": protocol,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER, "version": env!("CARGO_PKG_VERSION") },
    })
}

/// What of [`SERVED`] an agent with these switches has, in MCP's shape. The description and schema
/// are the ones the adi loop advertises, untouched — the whole point of serving the shared table
/// rather than a second one.
fn served_tools(code: CodeTools) -> Vec<Value> {
    tools::offered(code)
        .filter(|spec| SERVED.contains(&spec.name))
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": (spec.schema)(),
            })
        })
        .collect()
}

/// Run one tool call.
///
/// A tool that fails comes back as a *successful* JSON-RPC result carrying `isError: true`, because
/// that is what the failure is: an answer the model is expected to read and correct itself from,
/// not a breakdown of the request. This mirrors [`tools::run`]'s own contract, where the error half
/// of the result is the tool's reply.
fn call(request: &Value, ctx: &Ctx<'_>) -> std::result::Result<Value, String> {
    let name = request
        .pointer("/params/name")
        .and_then(Value::as_str)
        .ok_or_else(|| "a tools/call needs params.name".to_string())?;
    // A code tool this agent does not have is no tool at all, exactly as it was not listed.
    if !SERVED.contains(&name) || !ctx.code.offers(name) {
        return Err(format!("no tool named {name}"));
    }
    // Absent arguments are an empty object, not a failure: a tool with only optional fields is
    // legitimately called with none, and each tool already reports its own missing arguments in the
    // terms the model needs to fix them.
    let arguments = request
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let (text, is_error) = match tools::execute(name, &arguments, ctx) {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    };
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-mcp-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// Drive a whole session over the transport and read back the replies, one per line — as an
    /// agent with neither code tool, which is what an agent is until its definition says otherwise.
    fn talk(requests: &str) -> Vec<Value> {
        talk_as(CodeTools::default(), requests)
    }

    /// The same, as an agent with the given code tools switched on.
    fn talk_as(code: CodeTools, requests: &str) -> Vec<Value> {
        let dir = scratch("talk");
        let mut out = Vec::new();
        serve(
            "watcher",
            "conv-1",
            &dir,
            &SessionStore::new(dir.join("sessions")),
            Standing {
                unattended: false,
                code,
            },
            std::io::Cursor::new(requests.as_bytes()),
            &mut out,
        )
        .expect("serve");
        let _ = std::fs::remove_dir_all(&dir);
        String::from_utf8(out)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSON-RPC line"))
            .collect()
    }

    /// The default, and the whole point of the arrangement: an agent that said nothing about tools
    /// gets *no* built-ins — not the file tools, not the web, not subagents, and none of the cloud
    /// surfaces the engine's CLI happens to ship this month. What it has is this run's own server.
    #[test]
    fn an_agent_that_names_no_tools_gets_none_of_the_engines_own() {
        let scope = scope_tools(None);

        assert_eq!(scope.builtins, "", "empty is the CLI's spelling of 'none'");
        assert_eq!(
            scope.allowed, "mcp__adi",
            "ours is the one grant nobody asks for"
        );
    }

    /// The grant: an agent's own list is what switches a built-in on, and it does both jobs at once
    /// — the tool has to exist, and calling it must not stop to ask.
    #[test]
    fn an_agents_own_list_is_the_grant_and_never_buys_a_second_shell() {
        let scope = scope_tools(Some("Read,Edit,Workflow"));
        let available: Vec<&str> = scope.builtins.split(',').collect();

        assert_eq!(available, ["Read", "Edit", "Workflow"], "named, so present");
        assert!(
            scope.allowed.split(',').any(|t| t == "Read"),
            "{}",
            scope.allowed
        );
        assert!(
            scope.allowed.split(',').any(|t| t == "mcp__adi"),
            "{}",
            scope.allowed
        );
        for off in ["CronCreate", "Skill", "WebFetch", "Task", "ToolSearch"] {
            assert!(
                !available.contains(&off),
                "{off} was not asked for: {}",
                scope.builtins
            );
        }

        let scope = scope_tools(Some("Read Edit Write Bash Glob"));
        for shell in ENGINE_SHELL_TOOLS {
            assert!(
                !scope.builtins.split(',').any(|t| t == *shell),
                "{shell} is never grantable: {}",
                scope.builtins
            );
        }
        assert_eq!(scope.builtins, "Read,Edit,Write,Glob");
    }

    /// A scoped rule is two things at once: a permission written exactly as the agent wrote it, and
    /// the name of the tool it permits. Splitting a list on whitespace alone would cut `Bash(git *)`
    /// in half — and the entry that survived would grant a tool nobody named.
    #[test]
    fn a_scoped_rule_keeps_its_scope_where_it_is_a_rule_and_loses_it_where_it_is_a_name() {
        let scope = scope_tools(Some("Edit(src/**) WebFetch(domain:docs.rs)"));

        assert_eq!(scope.builtins, "Edit,WebFetch");
        assert!(
            scope.allowed.split(',').any(|t| t == "Edit(src/**)"),
            "the rule survives whole: {}",
            scope.allowed
        );

        let scope = scope_tools(Some("Bash(git *),Read"));
        assert_eq!(scope.builtins, "Read");
        assert!(!scope.allowed.contains("Bash"), "{}", scope.allowed);
    }

    #[test]
    fn the_grant_is_not_repeated_and_empty_entries_are_not_tools() {
        let scope = scope_tools(Some("Read,,mcp__adi, "));

        assert_eq!(scope.builtins, "Read", "an MCP name is not a built-in");
        assert_eq!(
            scope
                .allowed
                .split(',')
                .filter(|t| *t == "mcp__adi")
                .count(),
            1
        );
    }

    /// The handshake agrees with the client rather than insisting on a version of its own, and says
    /// plainly that tools are all it has.
    #[test]
    fn initialize_echoes_the_clients_protocol_version() {
        let replies = talk(
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
        );
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
        assert!(replies[0]["result"]["capabilities"]["tools"].is_object());
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], SERVER);

        let bare = talk(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#);
        assert_eq!(bare[0]["result"]["protocolVersion"], DEFAULT_PROTOCOL);
    }

    /// The rule that is easiest to get wrong and hardest to notice: a notification has no `id`, and
    /// answering one is a protocol violation. `notifications/initialized` arrives on every single
    /// connection, so getting this wrong breaks every run rather than an unlucky one.
    #[test]
    fn a_notification_is_answered_with_silence() {
        let replies = talk(
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n\
             {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n",
        );
        assert_eq!(replies.len(), 1, "only the ping is answered: {replies:?}");
        assert_eq!(replies[0]["id"], 1);
    }

    /// Blank lines and unparseable ones cost the connection nothing — there is no id to answer
    /// under, and killing the server would take the whole run with it.
    #[test]
    fn junk_on_the_wire_does_not_end_the_session() {
        let replies = talk(
            "\n\
             not json at all\n\
             {\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n",
        );
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["id"], 7);
    }

    /// What is listed is the shared table, filtered — not a second description written here that
    /// could drift from the one the adi loop advertises.
    #[test]
    fn only_the_served_tools_are_listed_and_they_are_the_shared_ones() {
        let replies = talk(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let listed = replies[0]["result"]["tools"]
            .as_array()
            .expect("tools")
            .clone();
        let names: Vec<&str> = listed
            .iter()
            .map(|t| t["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            names,
            ["Bash", "Await", "Ask"],
            "the file tools stay the CLI's own, and a code tool nobody switched on is not there"
        );

        let bash = tools::TOOLS
            .iter()
            .find(|t| t.name == "Bash")
            .expect("Bash is a shared tool");
        let served = listed.iter().find(|t| t["name"] == "Bash").expect("served");
        assert_eq!(served["description"], bash.description);
        assert_eq!(served["inputSchema"], (bash.schema)());
    }

    /// The code tools are an agent's to switch on, one by one: each is listed only for an agent
    /// that has it, and calling one it does not have is calling a tool that does not exist —
    /// neither is advertised and then refused.
    #[cfg(unix)]
    #[test]
    fn a_code_tool_is_listed_and_callable_only_for_an_agent_that_switched_it_on() {
        let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let names = |code: CodeTools| -> Vec<String> {
            talk_as(code, list)[0]["result"]["tools"]
                .as_array()
                .expect("tools")
                .iter()
                .map(|t| t["name"].as_str().expect("name").to_string())
                .collect()
        };
        let python = CodeTools { python: true, typescript: false };
        let typescript = CodeTools { python: false, typescript: true };
        assert_eq!(names(python), ["Bash", "PyCode", "Await", "Ask"]);
        assert_eq!(names(typescript), ["Bash", "TsCode", "Await", "Ask"]);
        assert_eq!(names(CodeTools::BOTH), SERVED);

        let call = |name: &str| {
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": name, "arguments": { "code": "1" } },
            })
            .to_string()
        };
        let refused = talk_as(python, &call("TsCode"));
        assert_eq!(refused[0]["error"]["code"], -32601, "{:?}", refused[0]);
        let refused = talk(&call("PyCode"));
        assert_eq!(refused[0]["error"]["code"], -32601, "{:?}", refused[0]);
    }

    /// `TsCode` crosses the wire the same way, with an interpreter of its own: what a cell
    /// declared is there for the next, and an exception that escapes is `isError`.
    #[cfg(unix)]
    #[test]
    fn tscode_is_served_keeps_state_between_calls_and_flags_an_exception() {
        let call = |id: u32, code: &str| {
            json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": "TsCode", "arguments": { "code": code } },
            })
            .to_string()
        };
        let replies = talk_as(
            CodeTools::BOTH,
            &[call(1, "const x: number = 20;"), call(2, "x + 1;"), call(3, "(null as any).y;")].join("\n"),
        );
        let text = |i: usize| {
            replies[i]["result"]["content"][0]["text"]
                .as_str()
                .expect("text")
                .to_string()
        };

        assert_eq!(replies[0]["result"]["isError"], false, "{:?}", replies[0]);
        assert_eq!(text(1), "21", "the second call saw what the first one declared");
        assert_eq!(replies[2]["result"]["isError"], true, "{:?}", replies[2]);
        assert!(text(2).contains("TypeError"), "{}", text(2));
    }

    /// `PyCode` crosses the wire as `Bash` does, and one server's calls share one interpreter: the
    /// second call reads what the first one set. An exception that escapes is `isError`.
    #[cfg(unix)]
    #[test]
    fn pycode_is_served_keeps_state_between_calls_and_flags_an_exception() {
        let call = |id: u32, code: &str| {
            json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": "PyCode", "arguments": { "code": code } },
            })
            .to_string()
        };
        let replies = talk_as(CodeTools::BOTH, &[call(1, "x = 20"), call(2, "x + 1"), call(3, "x / 0")].join("\n"));
        let text = |i: usize| {
            replies[i]["result"]["content"][0]["text"]
                .as_str()
                .expect("text")
                .to_string()
        };

        assert_eq!(replies[0]["result"]["isError"], false, "{:?}", replies[0]);
        assert_eq!(text(1), "21", "the second call saw the first one's variable");
        assert_eq!(replies[2]["result"]["isError"], true, "{:?}", replies[2]);
        assert!(text(2).contains("ZeroDivisionError"), "{}", text(2));
    }

    #[test]
    fn a_call_runs_the_tool_and_carries_its_output() {
        let replies = talk(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"Bash","arguments":{"command":"echo mcp-works"}}}"#,
        );
        let result = &replies[0]["result"];
        assert_eq!(result["isError"], false);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .expect("text")
                .contains("mcp-works"),
            "{result}"
        );
    }

    /// A failing tool is a *successful* call whose content is the failure — the model reads it and
    /// corrects itself. Reporting it as a JSON-RPC error would tell the client the request broke.
    #[test]
    fn a_failing_tool_is_a_result_with_is_error_not_a_protocol_error() {
        let replies = talk(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"Bash","arguments":{"command":"exit 3"}}}"#,
        );
        assert!(replies[0].get("error").is_none(), "{:?}", replies[0]);
        let result = &replies[0]["result"];
        assert!(result["content"][0]["text"].is_string());

        let unknown = talk(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"Write","arguments":{}}}"#,
        );
        assert_eq!(unknown[0]["error"]["code"], -32601);
        assert!(
            unknown[0]["error"]["message"]
                .as_str()
                .expect("message")
                .contains("Write"),
        );
    }

    #[test]
    fn an_unknown_method_is_method_not_found() {
        let replies = talk(r#"{"jsonrpc":"2.0","id":5,"method":"resources/list"}"#);
        assert_eq!(replies[0]["error"]["code"], -32601);
    }

    /// A real `python3` process, speaking MCP stdio for real — the point is not to test
    /// [`crate::mcp_servers::resolve`] or [`config`] against each other's assumptions, but against
    /// an independent third implementation of the protocol, the same one the Claude CLI would be.
    ///
    /// # What it exercises
    /// A server definition's `command`/`args`/`env` reach [`adi_mcp::McpServers`] on disk, get
    /// resolved by [`crate::mcp_servers::resolve`] against this run's attached secrets, get folded
    /// into this module's own `--mcp-config` JSON by [`config`] under the server's own id (next to
    /// `"adi"`), and *that exact command* — read back out of the JSON, not the one this test wrote
    /// — launches a process that completes a real `initialize` + `tools/list` round trip.
    /// A minimal [`Session`] double for the one test below that needs to hand [`config`] a real
    /// one — not reused from the other test modules' own `FakeSession`s, each of which lives
    /// behind its own module boundary.
    struct FakeSession {
        log: std::path::PathBuf,
    }
    impl Session for FakeSession {
        fn id(&self) -> &'static str {
            "conv-1"
        }
        fn agent(&self) -> &'static str {
            "watcher"
        }
        fn has_started(&self) -> bool {
            false
        }
        fn state(&self) -> Option<Value> {
            None
        }
        fn set_state(&self, _value: Value) -> crate::error::Result<()> {
            Ok(())
        }
        fn log_path(&self) -> &std::path::Path {
            &self.log
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // the point is to drive a real subprocess end to end
    fn a_declared_stdio_server_resolves_lands_in_the_config_and_really_speaks_mcp() {
        let dir = scratch("stdio-e2e");
        let script = dir.join("echo_server.py");
        std::fs::write(
            &script,
            "import sys, json\n\
             for line in sys.stdin:\n\
             \x20   line = line.strip()\n\
             \x20   if not line:\n\
             \x20       continue\n\
             \x20   req = json.loads(line)\n\
             \x20   method = req.get('method')\n\
             \x20   if method == 'initialize':\n\
             \x20       result = {'protocolVersion': req.get('params', {}).get('protocolVersion', '2025-11-25'),\n\
             \x20                 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'echo-test', 'version': '0'}}\n\
             \x20   elif method == 'tools/list':\n\
             \x20       result = {'tools': [{'name': 'Echo', 'description': 'echoes back', 'inputSchema': {'type': 'object'}}]}\n\
             \x20   else:\n\
             \x20       continue\n\
             \x20   print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)\n",
        )
        .expect("write fixture server");

        let store_config = adi_config::Config::with_root(dir.join("store"));
        adi_mcp::McpServers::with_config(store_config.clone())
            .save(
                "echo",
                adi_mcp::Manifest::new(
                    "echo",
                    adi_mcp::Transport::Stdio {
                        command: "python3".to_string(),
                        args: vec![script.display().to_string()],
                        env: std::collections::BTreeMap::new(),
                    },
                ),
            )
            .expect("save definition");

        let resolved = crate::mcp_servers::resolve(&store_config, &["echo".to_string()], &[]);
        assert_eq!(resolved.len(), 1, "the definition just saved should resolve with no secrets needed");

        let session = FakeSession { log: dir.join("conv-1.log") };
        let spec = RunSpec {
            credential: None,
            cwd: dir.clone(),
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
            mcp_servers: resolved,
        };

        let mcp_config_json = config(&spec, &session);
        let parsed: Value = serde_json::from_str(&mcp_config_json).expect("valid json");
        // ADI's own server is still there, untouched, right beside the agent's declared one.
        assert!(parsed["mcpServers"]["adi"].is_object(), "{mcp_config_json}");
        let echo = &parsed["mcpServers"]["echo"];
        assert_eq!(echo["type"], "stdio");
        assert_eq!(echo["command"], "python3");
        let real_command = echo["command"].as_str().expect("command").to_string();
        let real_args: Vec<String> = echo["args"]
            .as_array()
            .expect("args")
            .iter()
            .map(|v| v.as_str().expect("arg string").to_string())
            .collect();

        // Launch *exactly* what the config says to launch, and speak MCP to it for real.
        let mut child = std::process::Command::new(&real_command)
            .args(&real_args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn the fixture server");
        {
            use std::io::Write as _;
            let stdin = child.stdin.as_mut().expect("stdin");
            writeln!(
                stdin,
                r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-11-25"}}}}"#
            )
            .expect("write initialize");
            writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).expect("write tools/list");
        }
        let mut lines = {
            use std::io::{BufRead as _, BufReader};
            let stdout = child.stdout.take().expect("stdout");
            BufReader::new(stdout).lines()
        };
        let initialize_reply: Value =
            serde_json::from_str(&lines.next().expect("a reply").expect("read line")).expect("json");
        assert_eq!(initialize_reply["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(initialize_reply["result"]["serverInfo"]["name"], "echo-test");

        let list_reply: Value =
            serde_json::from_str(&lines.next().expect("a reply").expect("read line")).expect("json");
        assert_eq!(list_reply["result"]["tools"][0]["name"], "Echo");

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An http server's `config()` output carries the *rendered* header — `Bearer <token>`, not
    /// the prefix/raw split [`crate::mcp_servers::ResolvedHeader`] keeps internally for Codex's
    /// own bearer knob (see `backends::process::codex_mcp`). A Claude engine never sees that
    /// split; it sees exactly the header string it would send over the wire.
    #[test]
    fn a_declared_http_server_with_a_bearer_secret_renders_the_full_header() {
        let resolved = crate::mcp_servers::ResolvedMcpServer {
            id: "n8n".to_string(),
            transport: crate::mcp_servers::ResolvedTransport::Http {
                url: "http://n8n.adi/mcp-server/http".to_string(),
                headers: std::collections::BTreeMap::from([(
                    "Authorization".to_string(),
                    crate::mcp_servers::ResolvedHeader::Prefixed {
                        prefix: "Bearer ".to_string(),
                        raw: "tok-123".to_string(),
                    },
                )]),
            },
        };
        let json = server_json(&resolved);
        assert_eq!(json["type"], "http");
        assert_eq!(json["url"], "http://n8n.adi/mcp-server/http");
        assert_eq!(json["headers"]["Authorization"], "Bearer tok-123");
    }
}
