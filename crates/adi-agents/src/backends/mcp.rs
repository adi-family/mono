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
//! Only [`SERVED`] — `Bash` and `Await`. The runner disallows the CLI's built-in `Bash` (see
//! [`super::super::runner::detached`]) precisely so ours is the shell an agent gets: one shell per
//! *conversation*, where a `cd` and an `export` outlive the call and the turn (see [`super::shell`]).
//! The CLI's own `Read`/`Write`/`Edit`/`Glob`/`Grep` are left alone and keep serving that run —
//! they are better than ours (diff-aware edits, the CLI's own permission prompts), and nothing about
//! them needs to be a conversation-scoped thing.
//!
//! Our `Bash` keeps its name rather than being renamed out of the way. Measured against the CLI:
//! disallowing `Bash` does not touch `mcp__adi__Bash`, because the deny list matches whole tool
//! names and an MCP tool's name is server-qualified. So the vocabulary stays the one
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

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{Value, json};

use super::harness::tools::{self, Ctx};
use super::shell::Shell;
use crate::awaits::Awaits;
use crate::error::{Error, Result};
use crate::runner::{RunSpec, Session};

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
const SERVED: &[&str] = &["Bash", "Await"];

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
            "mcp",
            "--agent", session.agent(),
            "--session", session.id(),
            "--dir", spec.cwd.to_string_lossy(),
        ],
    });
    let mut servers = serde_json::Map::new();
    servers.insert(SERVER.to_string(), server);
    serde_json::json!({ "mcpServers": servers }).to_string()
}

/// The engine's own shell, switched off. Ours replaces it — see the module docs for why
/// that trade is worth making, and [`super::shell`] for what the engine's own `Bash` could
/// never do.
///
/// `BashOutput` and `KillShell` go with it: they exist only to poll and kill the background jobs the
/// built-in `Bash` starts, so leaving them enabled would advertise two tools that can no longer
/// refer to anything.
///
/// Unlike [`ENGINE_EXTRA_TOOLS`], an agent cannot opt back into these. It is not a preference: a
/// second shell would be a second conversation state, and the one thing this whole arrangement
/// exists to guarantee is that there is exactly one.
pub(crate) const ENGINE_SHELL_TOOLS: &[&str] = &["Bash", "BashOutput", "KillShell"];

/// Everything else the engine's CLI brings that an ADI agent was never asked whether it wanted —
/// **off unless the agent says otherwise**.
///
/// A Claude run arrives with the CLI's whole built-in surface, and a good deal of it reaches past
/// this machine or spends money on its own: cloud cron schedules, claude.ai design and trigger
/// surfaces, push notifications, skills, a second task tracker beside the platform's own. An agent
/// definition never mentioned any of it, so defaulting it *on* means every agent silently holds
/// powers nobody granted. Deny-by-default inverts that: the agent's own `allowed_tools` is the
/// grant, and an empty one means the file tools, the web tools, subagents, and ours.
///
/// `ToolSearch` is in here and looks load-bearing, because MCP tools arrive deferred and it is what
/// loads them. Measured: with `ToolSearch` denied, a run still calls `mcp__adi__Bash` perfectly
/// well — the deferral is a context optimisation, not a gate.
///
/// What is deliberately *not* here: `Read`/`Write`/`Edit`/`Glob`/`Grep` (the working surface, and
/// better than ours), `WebSearch`/`WebFetch` and `Agent` — several agents' own prompts require
/// those by name, and switching them off would break the workflow those prompts describe.
pub(crate) const ENGINE_EXTRA_TOOLS: &[&str] = &[
    // Cloud schedules, and the claude.ai surfaces.
    "CronCreate",
    "CronDelete",
    "CronList",
    "DesignSync",
    "RemoteTrigger",
    "PushNotification",
    // Harness machinery an ADI agent has its own answer for, or no use for.
    "Skill",
    "SendMessage",
    "ListAgents",
    "ReportFindings",
    "ScheduleWakeup",
    "EnterWorktree",
    "ExitWorktree",
    "LSP",
    "Monitor",
    "NotebookEdit",
    // The CLI's own task tracker — the platform has `adi-tasks`, and two is worse than either.
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskUpdate",
    // Deferred-tool loading (see above) and multi-agent fan-out, which is real money.
    "ToolSearch",
    "Workflow",
];

/// `existing` with `names` added, as the CLI's comma-separated tool list.
///
/// Additive rather than authoritative: an agent's own `--allowed-tools` is a decision somebody made
/// about that agent, and this has no business replacing it. Already-listed names are not repeated,
/// so re-running this over a list it already touched is a no-op rather than a slow leak.
fn with_tools(existing: Option<&str>, names: &[&str]) -> String {
    let mut listed: Vec<String> = existing
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
        .collect();
    for name in names {
        if !listed.iter().any(|entry| entry == name) {
            listed.push((*name).to_string());
        }
    }
    listed.join(",")
}

/// Grant this run's own MCP tools, and take the engine's shell away.
///
/// Both halves are required for either to be worth doing. Measured against the CLI: an MCP tool the
/// run has not been granted is *advertised and then refused* — the model sees it, calls it, and is
/// told it lacks permission — so registering the server without granting it produces an agent that
/// can see a shell it may not use. The server-level grant (`mcp__adi`) covers every tool the server
/// serves, so adding one later needs no change here.
pub(crate) fn scope_tools(allowed: Option<&str>, disallowed: Option<&str>) -> (String, String) {
    let grant = format!("mcp__{SERVER}");
    // The agent's own allow-list is the opt-in: naming an extra there takes it back off the deny
    // list. Removing it rather than relying on which flag the CLI weighs more heavily — a
    // precedence we would be guessing at, and one that could change under us.
    let opted_in: Vec<&str> = allowed
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect();
    let mut deny: Vec<&str> = ENGINE_SHELL_TOOLS.to_vec();
    deny.extend(
        ENGINE_EXTRA_TOOLS
            .iter()
            .copied()
            .filter(|extra| !opted_in.contains(extra)),
    );
    (
        with_tools(allowed, &[grant.as_str()]),
        with_tools(disallowed, &deny),
    )
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
    agent_dir: &Path,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<()> {
    let ctx = Ctx {
        cwd,
        // The conversation's shell, keyed by session id exactly as the adi loop keys it — so one
        // conversation has one shell whichever engine answered which turn.
        shell: Shell::new(agent_dir, conv),
        agent,
        conv,
        awaits: Awaits::open(),
        agent_dir,
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
        // A ping exists to prove the pipe is alive; an empty result is the whole of it.
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": served_tools() })),
        "tools/call" => call(request, ctx),
        other => Err(format!("no method {other}")),
    };

    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        // -32601 is JSON-RPC's "method not found", the only protocol-level error this server can
        // raise: a *tool* that fails is a successful call carrying `isError`, not a failed request.
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

/// [`SERVED`] in MCP's shape. The description and schema are the ones the adi loop advertises,
/// untouched — the whole point of serving the shared table rather than a second one.
fn served_tools() -> Vec<Value> {
    tools::TOOLS
        .iter()
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
    if !SERVED.contains(&name) {
        return Err(format!("no tool named {name}"));
    }
    // Absent arguments are an empty object, not a failure: a tool with only optional fields is
    // legitimately called with none, and each tool already reports its own missing arguments in the
    // terms the model needs to fix them.
    let arguments = request
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let (text, is_error) = match tools::run(name, &arguments, ctx) {
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

    /// Drive a whole session over the transport and read back the replies, one per line.
    fn talk(requests: &str) -> Vec<Value> {
        let dir = scratch("talk");
        let mut out = Vec::new();
        serve(
            "watcher",
            "conv-1",
            &dir,
            &dir,
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

    /// The default an agent gets when it has said nothing about tools: our shell in, the engine's
    /// shell out, and the CLI's extra surface off. What stays on is the working surface — an agent
    /// that never asked for anything still reads, writes, searches the web, and delegates.
    #[test]
    fn the_engines_extra_surface_is_off_unless_an_agent_asks_for_it() {
        let (allowed, disallowed) = scope_tools(None, None);
        let denied: Vec<&str> = disallowed.split(',').collect();

        assert!(allowed.split(',').any(|t| t == "mcp__adi"));
        for off in ["CronCreate", "RemoteTrigger", "Skill", "Workflow", "ToolSearch", "TaskCreate"] {
            assert!(denied.contains(&off), "{off} is off by default: {disallowed}");
        }
        // The line this draws, and the reason it is drawn there: these are named by agents' own
        // prompts, so switching them off would break the workflow those prompts describe.
        for on in ["Read", "Write", "Edit", "Glob", "Grep", "WebSearch", "WebFetch", "Agent"] {
            assert!(!denied.contains(&on), "{on} must stay available: {disallowed}");
        }
    }

    /// The opt-in: an agent that names an extra in its own `allowed_tools` gets it back. Taken off
    /// the deny list rather than left on both, so nothing depends on which flag the CLI weighs more.
    #[test]
    fn an_agent_can_ask_for_an_extra_back_and_never_for_a_second_shell() {
        let (allowed, disallowed) = scope_tools(Some("Read,Workflow,CronList"), None);
        let denied: Vec<&str> = disallowed.split(',').collect();

        assert!(!denied.contains(&"Workflow"), "asked for, so granted: {disallowed}");
        assert!(!denied.contains(&"CronList"), "asked for, so granted: {disallowed}");
        assert!(denied.contains(&"Skill"), "everything else stays off: {disallowed}");
        assert!(allowed.split(',').any(|t| t == "Read"), "its own list survives: {allowed}");

        // The shell is not a preference. An agent that names `Bash` — as one written before ours
        // existed well might — still gets ours, because a second shell is a second conversation
        // state and the whole arrangement exists to guarantee there is one.
        let (_, disallowed) = scope_tools(Some("Read,Edit,Write,Bash,Glob"), None);
        for shell in ENGINE_SHELL_TOOLS {
            assert!(
                disallowed.split(',').any(|t| t == *shell),
                "{shell} is never opt-in-able: {disallowed}"
            );
        }
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

        // A client that names no version still gets a usable answer.
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
        assert_eq!(names, SERVED, "the file tools stay the CLI's own");

        // Description and schema come from `tools::TOOLS` verbatim.
        let bash = tools::TOOLS
            .iter()
            .find(|t| t.name == "Bash")
            .expect("Bash is a shared tool");
        let served = listed.iter().find(|t| t["name"] == "Bash").expect("served");
        assert_eq!(served["description"], bash.description);
        assert_eq!(served["inputSchema"], (bash.schema)());
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

        // A tool we do not serve is a different thing: the request named something that isn't here.
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
}
