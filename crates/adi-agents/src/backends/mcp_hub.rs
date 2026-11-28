//! An agent's own MCP servers, reached from *this* process instead of by its engine.
//!
//! Everywhere else in this crate an agent's declared servers ([`crate::AgentManifest::mcp`]) are
//! somebody else's to talk to: they are reshaped into the engine's own configuration
//! ([`super::mcp::config`], `process::codex_mcp`) and the engine's CLI is the MCP client. A
//! code-only agent ([`CodeTools::only`](super::harness::tools::CodeTools)) has no such client. Its
//! engine is told about one tool — the interpreter — so that a server's tools are functions a
//! script calls rather than entries in the model's tool list, and something on this side of the
//! interpreter has to be the client those functions reach. This is it.
//!
//! # Lifetime
//!
//! A [`Hub`] belongs to whoever serves a conversation's tool calls — the MCP server an engine's
//! CLI spawned, or the adi loop's turn child — and lives as long as that process does. A server is
//! connected the first time a script uses it and kept until the hub is dropped, which is the same
//! lifetime an engine's own client gives it: a stdio server is a child of the process serving the
//! turn, and ends with it.
//!
//! # The two transports
//!
//! Written against MCP revision [`PROTOCOL`]
//! (<https://modelcontextprotocol.io/specification/2025-06-18/basic/transports>):
//!
//! - **stdio** — the server is a child process; each message is one line of JSON-RPC 2.0 on its
//!   stdin or stdout. Its stderr is the server's own log, kept only as a tail to quote when it
//!   will not start.
//! - **Streamable HTTP** — every message is a `POST` to one URL. The answer is either a JSON body
//!   or a `text/event-stream` in which the response is one of the events; a session id the server
//!   hands out at `initialize` is echoed on every later request.
//!
//! Only the client half of the protocol that a tool call needs is here: `initialize`,
//! `tools/list`, `tools/call`. No resources, no prompts, no sampling — and the handshake says so,
//! which is what lets a request a server sends *us* be declined rather than left hanging.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::mcp_servers::{ResolvedMcpServer, ResolvedTransport};

/// The protocol revision this client asks for. A server answers with the one it will speak, and
/// whatever it answers is accepted: the three methods used here have not changed shape across
/// the revisions in the wild.
const PROTOCOL: &str = "2025-06-18";

/// How much of a stdio server's stderr is kept to explain a failure.
const STDERR_TAIL: usize = 2_000;

/// How many `tools/list` pages are followed. A server that keeps handing out cursors past this is
/// not paginating, it is looping.
const MAX_PAGES: usize = 50;

/// The MCP servers one conversation's scripts may call. Cheap to clone — every clone is the same
/// set of connections.
#[derive(Clone, Default)]
pub(crate) struct Hub {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    servers: Vec<ResolvedMcpServer>,
    /// Where a stdio server is started: the run's own directory, as an engine's client does it.
    cwd: PathBuf,
    /// One client per server that has been used, each behind its own lock so a slow call to one
    /// server does not hold up a call to another.
    live: Mutex<BTreeMap<String, Arc<Mutex<Client>>>>,
}

/// What a `tools/call` came to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Outcome {
    /// The text of its content — every text item, in order.
    pub(crate) text: String,
    /// `structuredContent`, when the server gave one.
    pub(crate) value: Option<Value>,
    /// The server's own word that the call failed (`isError`), which is an answer and not a
    /// broken transport.
    pub(crate) is_error: bool,
}

impl Hub {
    /// The hub for an agent with these servers, none of them connected yet.
    pub(crate) fn new(servers: Vec<ResolvedMcpServer>, cwd: &Path) -> Self {
        Self {
            inner: Arc::new(Inner {
                servers,
                cwd: cwd.to_path_buf(),
                live: Mutex::default(),
            }),
        }
    }

    /// The ids of the servers there are, in the order the agent names them.
    pub(crate) fn ids(&self) -> Vec<String> {
        self.inner.servers.iter().map(|s| s.id.clone()).collect()
    }

    /// Every tool `server` offers, exactly as it describes them (`name`, `description`,
    /// `inputSchema`).
    ///
    /// # Errors
    /// A message for the model: no such server, one that would not start or answer, or the
    /// server's own refusal.
    pub(crate) fn list(&self, server: &str, deadline: Instant) -> Result<Value, String> {
        let client = self.client(server)?;
        let mut client = lock(&client);
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
            let page = client.request("tools/list", &params, deadline)?;
            if let Some(found) = page.get("tools").and_then(Value::as_array) {
                tools.extend(found.iter().cloned());
            }
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|next| !next.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(Value::Array(tools))
    }

    /// Call `tool` on `server`.
    ///
    /// # Errors
    /// A message for the model when the call could not be made at all. A call the server ran and
    /// reported as failed is an [`Outcome`] with `is_error` set.
    pub(crate) fn call(
        &self,
        server: &str,
        tool: &str,
        arguments: &Value,
        deadline: Instant,
    ) -> Result<Outcome, String> {
        let client = self.client(server)?;
        let result = lock(&client).request(
            "tools/call",
            &json!({ "name": tool, "arguments": arguments }),
            deadline,
        )?;
        Ok(Outcome {
            text: content_text(&result),
            value: result
                .get("structuredContent")
                .filter(|value| !value.is_null())
                .cloned(),
            is_error: result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// The client for `id`, made on first use and not connected until its first request.
    fn client(&self, id: &str) -> Result<Arc<Mutex<Client>>, String> {
        let Some(server) = self.inner.servers.iter().find(|s| s.id == id) else {
            let known = self.ids();
            return Err(format!(
                "no MCP server named {id} is attached to this agent — {}",
                if known.is_empty() {
                    "none is".to_string()
                } else {
                    format!("the ones that are: {}", known.join(", "))
                }
            ));
        };
        let mut live = lock(&self.inner.live);
        Ok(live
            .entry(id.to_string())
            .or_insert_with(|| {
                Arc::new(Mutex::new(Client {
                    server: server.clone(),
                    cwd: self.inner.cwd.clone(),
                    link: None,
                }))
            })
            .clone())
    }
}

/// A poisoned lock is still the data: a call that panicked mid-request left a connection that is
/// at worst out of step, which the next request finds out and reports.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The text of a `tools/call` result: its text items in order, and a word standing in for each
/// item that is something else — an image a script cannot use is still worth knowing was sent.
fn content_text(result: &Value) -> String {
    let Some(items) = result.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => parts.push(
                item.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            Some("resource") => match item.pointer("/resource/text").and_then(Value::as_str) {
                Some(text) => parts.push(text.to_string()),
                None => parts.push("[a resource that is not text]".to_string()),
            },
            Some(other) => parts.push(format!("[{other} content]")),
            None => {}
        }
    }
    parts.join("\n")
}

/// One server, and the connection to it once there is one.
struct Client {
    server: ResolvedMcpServer,
    cwd: PathBuf,
    link: Option<Link>,
}

enum Link {
    Stdio(StdioLink),
    Http(HttpLink),
}

impl Client {
    /// Send one request and return its `result`, connecting first if this is the first.
    ///
    /// A connection that broke under the request is dropped, so the next request starts a new one
    /// instead of failing the same way for the rest of the turn.
    fn request(
        &mut self,
        method: &str,
        params: &Value,
        deadline: Instant,
    ) -> Result<Value, String> {
        let id = self.server.id.clone();
        if self.link.is_none() {
            self.link = Some(
                self.connect(deadline)
                    .map_err(|e| format!("couldn't connect to the MCP server {id}: {e}"))?,
            );
        }
        let Some(link) = self.link.as_mut() else {
            return Err(format!("couldn't connect to the MCP server {id}"));
        };
        match link.exchange(method, params, deadline) {
            Ok(result) => Ok(result),
            Err(Failed::Refused(message)) => {
                Err(format!("the MCP server {id} refused {method}: {message}"))
            }
            Err(Failed::Broken(message)) => {
                self.link = None;
                Err(format!(
                    "the MCP server {id} did not answer {method}: {message}"
                ))
            }
        }
    }

    /// Open the transport and shake hands.
    fn connect(&self, deadline: Instant) -> Result<Link, String> {
        let mut link = match &self.server.transport {
            ResolvedTransport::Stdio { command, args, env } => {
                Link::Stdio(StdioLink::start(command, args, env, &self.cwd)?)
            }
            ResolvedTransport::Http { url, headers } => Link::Http(HttpLink::new(
                url,
                headers
                    .iter()
                    .map(|(name, value)| (name.clone(), value.rendered()))
                    .collect(),
            )?),
        };
        let hello = json!({
            "protocolVersion": PROTOCOL,
            // Nothing: no roots, no sampling, no elicitation. A server that reads this knows not
            // to ask for any of them.
            "capabilities": {},
            "clientInfo": { "name": "adi", "version": env!("CARGO_PKG_VERSION") },
        });
        let welcome = link
            .exchange("initialize", &hello, deadline)
            .map_err(Failed::into_message)?;
        if let Link::Http(http) = &mut link
            && let Some(version) = welcome.get("protocolVersion").and_then(Value::as_str)
        {
            http.protocol = Some(version.to_string());
        }
        link.notify("notifications/initialized");
        Ok(link)
    }
}

/// Why a request got no result.
enum Failed {
    /// The server answered with a JSON-RPC error. The connection is fine.
    Refused(String),
    /// Nothing usable came back: the process died, the request timed out, the stream ended.
    Broken(String),
}

impl Failed {
    fn into_message(self) -> String {
        match self {
            Self::Refused(message) | Self::Broken(message) => message,
        }
    }
}

impl Link {
    fn exchange(
        &mut self,
        method: &str,
        params: &Value,
        deadline: Instant,
    ) -> Result<Value, Failed> {
        match self {
            Self::Stdio(link) => link.exchange(method, params, deadline),
            Self::Http(link) => link.exchange(method, params, deadline),
        }
    }

    /// A notification: sent, and nothing waited for. One that cannot be sent is found out by the
    /// request that follows it.
    fn notify(&mut self, method: &str) {
        let message = json!({ "jsonrpc": "2.0", "method": method });
        match self {
            Self::Stdio(link) => {
                let _ = link.send(&message);
            }
            Self::Http(link) => {
                let _ = link.post(&message, Instant::now() + Duration::from_secs(10));
            }
        }
    }
}

/// The `result` of a JSON-RPC response, or the error it carried instead.
fn result_of(response: &Value) -> Result<Value, Failed> {
    if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map_or_else(|| error.to_string(), str::to_string);
        return Err(Failed::Refused(message));
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

/// How long is left until `deadline`, or a failure saying there is none.
fn left(deadline: Instant) -> Result<Duration, Failed> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(Failed::Broken(
            "the call's deadline passed first".to_string(),
        ));
    }
    Ok(left)
}

/// A server that is a child process.
struct StdioLink {
    child: Child,
    stdin: ChildStdin,
    /// Every message the server wrote, parsed, in order.
    incoming: mpsc::Receiver<Value>,
    /// The end of what it wrote to stderr.
    stderr: Arc<Mutex<String>>,
    next: u64,
}

impl StdioLink {
    fn start(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if cwd.is_dir() {
            cmd.current_dir(cwd);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("couldn't start `{command}`: {e}"))?;
        let (Some(stdin), Some(stdout), Some(errors)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err(format!("couldn't open `{command}`'s pipes"));
        };

        let (tx, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // A line that is not JSON is a server logging to the wrong stream. Skipped: the
                // messages around it are still messages.
                if let Ok(message) = serde_json::from_str::<Value>(&line)
                    && tx.send(message).is_err()
                {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let kept = Arc::clone(&stderr);
        std::thread::spawn(move || {
            for line in BufReader::new(errors).lines() {
                let Ok(line) = line else { break };
                let mut tail = lock(&kept);
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > STDERR_TAIL * 2 {
                    let mut from = tail.len() - STDERR_TAIL;
                    while !tail.is_char_boundary(from) {
                        from += 1;
                    }
                    *tail = tail[from..].to_string();
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            incoming,
            stderr,
            next: 0,
        })
    }

    fn send(&mut self, message: &Value) -> std::io::Result<()> {
        writeln!(self.stdin, "{message}")?;
        self.stdin.flush()
    }

    fn exchange(
        &mut self,
        method: &str,
        params: &Value,
        deadline: Instant,
    ) -> Result<Value, Failed> {
        self.next += 1;
        let id = self.next;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(e) = self.send(&request) {
            return Err(Failed::Broken(format!("{e}{}", self.said())));
        }
        loop {
            let message = match self.incoming.recv_timeout(left(deadline)?) {
                Ok(message) => message,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(Failed::Broken(
                        "the call's deadline passed first".to_string(),
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Failed::Broken(format!("the server exited{}", self.said())));
                }
            };
            if message.get("method").is_some() {
                // The server asking *us* for something. Nothing was offered at `initialize`, so
                // the answer is no — but it is an answer, because a server waiting on one would
                // otherwise never get round to ours.
                if let Some(theirs) = message.get("id").filter(|id| !id.is_null()) {
                    let reply = if message.get("method").and_then(Value::as_str) == Some("ping") {
                        json!({ "jsonrpc": "2.0", "id": theirs, "result": {} })
                    } else {
                        json!({
                            "jsonrpc": "2.0",
                            "id": theirs,
                            "error": { "code": -32601, "message": "this client offers no such method" },
                        })
                    };
                    let _ = self.send(&reply);
                }
                continue;
            }
            // A response to something else — a request that timed out earlier — is nobody's now.
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                return result_of(&message);
            }
        }
    }

    /// What the server wrote to stderr, as the end of a sentence about it failing.
    fn said(&self) -> String {
        let tail = lock(&self.stderr);
        let tail = tail.trim();
        if tail.is_empty() {
            String::new()
        } else {
            format!(" — it said:\n{tail}")
        }
    }
}

impl Drop for StdioLink {
    fn drop(&mut self) {
        // Its stdin closing is the protocol's own way of ending a stdio server; the kill is for
        // one that does not take the hint.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A server reached over Streamable HTTP.
struct HttpLink {
    client: reqwest::blocking::Client,
    url: String,
    headers: Vec<(String, String)>,
    /// The session the server opened at `initialize`, echoed on every later request.
    session: Option<String>,
    /// The revision agreed at `initialize`, sent as a header from then on.
    protocol: Option<String>,
    next: u64,
}

impl HttpLink {
    fn new(url: &str, headers: Vec<(String, String)>) -> Result<Self, String> {
        // reqwest is built without a crypto provider of its own (see the workspace manifest), so
        // one is installed before the first client. A second install is refused and that is fine.
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let client = reqwest::blocking::Client::builder()
            // Per request, below: the only deadline that means anything here is the call's own.
            .timeout(None)
            .build()
            .map_err(|e| format!("couldn't build an HTTP client: {e}"))?;
        Ok(Self {
            client,
            url: url.to_string(),
            headers,
            session: None,
            protocol: None,
            next: 0,
        })
    }

    /// `POST` one message and hand back the response, unread.
    fn post(
        &mut self,
        message: &Value,
        deadline: Instant,
    ) -> Result<reqwest::blocking::Response, Failed> {
        let mut request = self
            .client
            .post(&self.url)
            .timeout(left(deadline)?)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        for (name, value) in &self.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(session) = &self.session {
            request = request.header("Mcp-Session-Id", session.as_str());
        }
        if let Some(protocol) = &self.protocol {
            request = request.header("MCP-Protocol-Version", protocol.as_str());
        }
        let response = request
            .body(message.to_string())
            .send()
            .map_err(|e| Failed::Broken(e.to_string()))?;
        if let Some(session) = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|value| value.to_str().ok())
        {
            self.session = Some(session.to_string());
        }
        Ok(response)
    }

    fn exchange(
        &mut self,
        method: &str,
        params: &Value,
        deadline: Instant,
    ) -> Result<Value, Failed> {
        self.next += 1;
        let id = self.next;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let response = self.post(&request, deadline)?;
        let status = response.status();
        if !status.is_success() {
            let body: String = response
                .text()
                .unwrap_or_default()
                .chars()
                .take(500)
                .collect();
            return Err(Failed::Broken(format!("HTTP {status} {}", body.trim())));
        }
        let streamed = response
            .headers()
            .get("Content-Type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|kind| kind.starts_with("text/event-stream"));
        if !streamed {
            let body: Value = response
                .json()
                .map_err(|e| Failed::Broken(format!("the answer was not JSON: {e}")))?;
            // One response, or — from a server on an older revision — a batch holding it.
            let ours = match &body {
                Value::Array(batch) => batch
                    .iter()
                    .find(|message| message.get("id").and_then(Value::as_u64) == Some(id)),
                message => Some(message),
            };
            return ours.map_or_else(
                || {
                    Err(Failed::Broken(
                        "the answer did not include this request's response".to_string(),
                    ))
                },
                result_of,
            );
        }

        // An event stream: each event's `data` lines are one JSON-RPC message, and the response
        // to this request is one of them — possibly behind notifications about its progress. Read
        // only as far as that event: the server is free to keep the stream open afterwards.
        let mut data = String::new();
        for line in BufReader::new(response).lines() {
            let line = line.map_err(|e| Failed::Broken(e.to_string()))?;
            if let Some(piece) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(piece.strip_prefix(' ').unwrap_or(piece));
                continue;
            }
            if !line.is_empty() {
                continue;
            }
            let event = std::mem::take(&mut data);
            if let Ok(message) = serde_json::from_str::<Value>(&event)
                && message.get("method").is_none()
                && message.get("id").and_then(Value::as_u64) == Some(id)
            {
                return result_of(&message);
            }
        }
        Err(Failed::Broken(
            "the stream ended without this request's response".to_string(),
        ))
    }
}

/// A stand-in MCP server for tests — here rather than in `tests` because the tests of a
/// code-only cell, which reaches a server through a script, need the same one.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    /// A stdio MCP server small enough to read: it answers the handshake, lists two tools, and
    /// runs them. `count` is there to show the process — and so its state — outlives a call;
    /// `ask-first` sends the client a request of its own before answering, as a server asking
    /// for sampling would.
    const SERVER: &str = r#"
import json, sys
calls = 0
def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()
print("starting up", file=sys.stderr, flush=True)
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message or "method" not in message:
        continue
    method, params, id = message["method"], message.get("params") or {}, message["id"]
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": params["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "fake", "version": "1"}}})
    elif method == "tools/list":
        if not params.get("cursor"):
            send({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "echo", "description": "Say it back", "inputSchema": {"type": "object"}}], "nextCursor": "more"}})
        else:
            send({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "count", "description": "How many calls", "inputSchema": {"type": "object"}}]}})
    elif method == "tools/call":
        calls += 1
        name, args = params["name"], params.get("arguments") or {}
        if name == "echo":
            send({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "you said " + args.get("text", "")}, {"type": "image", "data": "", "mimeType": "image/png"}]}})
        elif name == "count":
            send({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": str(calls)}], "structuredContent": {"calls": calls}}})
        elif name == "ask-first":
            send({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage", "params": {}})
            answer = json.loads(sys.stdin.readline())
            send({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "client said " + str(answer["error"]["code"])}]}})
        elif name == "fail":
            send({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "no such workflow"}], "isError": True}})
        elif name == "die":
            print("going down", file=sys.stderr, flush=True)
            sys.exit(3)
        else:
            send({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "unknown tool " + name}})
    else:
        send({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "no method " + method}})
"#;

    /// A hub with that server attached as `fake`, its script written into `dir`.
    pub(crate) fn stdio_hub(dir: &Path) -> Hub {
        let script = dir.join("mcp-fake-server.py");
        std::fs::write(&script, SERVER).expect("server script");
        Hub::new(
            vec![ResolvedMcpServer {
                id: "fake".to_string(),
                transport: ResolvedTransport::Stdio {
                    command: "python3".to_string(),
                    args: vec![script.display().to_string()],
                    env: BTreeMap::new(),
                },
            }],
            dir,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fake::stdio_hub;
    use super::*;
    use crate::mcp_servers::ResolvedHeader;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-mcp-hub-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(20)
    }

    /// The whole of what a script needs from a stdio server: its tools across pages, a call's
    /// text, a structured result, and the same process answering the second call as the first.
    #[test]
    fn a_stdio_server_is_listed_called_and_kept_between_calls() {
        let dir = scratch("stdio");
        let hub = stdio_hub(&dir);
        assert_eq!(hub.ids(), ["fake"]);

        let tools = hub.list("fake", soon()).expect("list");
        let names: Vec<&str> = tools
            .as_array()
            .expect("an array")
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["echo", "count"], "both pages");

        let said = hub
            .call("fake", "echo", &json!({ "text": "hi" }), soon())
            .expect("echo");
        assert_eq!(said.text, "you said hi\n[image content]");
        assert_eq!(said.value, None);
        assert!(!said.is_error);

        let counted = hub
            .call("fake", "count", &json!({}), soon())
            .expect("count");
        assert_eq!(counted.text, "2", "the process that ran echo ran this");
        assert_eq!(counted.value, Some(json!({ "calls": 2 })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Three ways a call does not go well, each told apart: the server ran it and says it failed,
    /// the server refuses the request, and the server asks the client for something this client
    /// does not offer — which must be declined, not left waiting.
    #[test]
    fn a_failed_call_a_refused_one_and_a_request_from_the_server_are_each_answered() {
        let dir = scratch("stdio-errors");
        let hub = stdio_hub(&dir);

        let failed = hub
            .call("fake", "fail", &json!({}), soon())
            .expect("it ran");
        assert!(failed.is_error);
        assert_eq!(failed.text, "no such workflow");

        let refused = hub
            .call("fake", "nope", &json!({}), soon())
            .expect_err("a JSON-RPC error");
        assert!(
            refused.contains("refused tools/call: unknown tool nope"),
            "{refused}"
        );

        let asked = hub
            .call("fake", "ask-first", &json!({}), soon())
            .expect("answered once the server's own request was declined");
        assert_eq!(asked.text, "client said -32601");

        let nobody = hub
            .call("other", "echo", &json!({}), soon())
            .expect_err("not attached");
        assert!(nobody.contains("no MCP server named other"), "{nobody}");
        assert!(nobody.contains("fake"), "what there is, is named: {nobody}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A server that dies under a call says why in the call's error, and the next call starts a
    /// new one rather than failing for the rest of the turn.
    #[test]
    fn a_server_that_dies_is_reported_and_started_again_by_the_next_call() {
        let dir = scratch("stdio-dies");
        let hub = stdio_hub(&dir);

        let died = hub
            .call("fake", "die", &json!({}), soon())
            .expect_err("it exited");
        assert!(died.contains("the server exited"), "{died}");
        assert!(died.contains("going down"), "with what it said: {died}");

        let again = hub
            .call("fake", "count", &json!({}), soon())
            .expect("a new process");
        assert_eq!(again.text, "1", "which has seen one call");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A command that is not on the machine is said in those words.
    #[test]
    fn a_server_that_cannot_be_started_says_so() {
        let dir = scratch("stdio-missing");
        let hub = Hub::new(
            vec![ResolvedMcpServer {
                id: "gone".to_string(),
                transport: ResolvedTransport::Stdio {
                    command: "adi-no-such-mcp-server".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
            }],
            &dir,
        );
        let err = hub.list("gone", soon()).expect_err("nothing to start");
        assert!(
            err.contains("couldn't connect to the MCP server gone"),
            "{err}"
        );
        assert!(err.contains("adi-no-such-mcp-server"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One request as the fake HTTP server below saw it.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        authorization: Option<String>,
        session: Option<String>,
        protocol: Option<String>,
    }

    /// An HTTP MCP server on a local port: it answers `initialize` with a JSON body and a
    /// session id, a notification with `202`, and everything else as an event stream that leads
    /// with a progress notification — the two shapes a real server may choose between.
    fn http_server() -> (String, Arc<Mutex<Vec<Seen>>>) {
        use std::io::Read as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/mcp", listener.local_addr().expect("addr"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut headers = BTreeMap::new();
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }
                let length: usize = headers
                    .get("content-length")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0_u8; length];
                let _ = reader.read_exact(&mut body);
                let message: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let method = message
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                lock(&log).push(Seen {
                    method: method.clone(),
                    authorization: headers.get("authorization").cloned(),
                    session: headers.get("mcp-session-id").cloned(),
                    protocol: headers.get("mcp-protocol-version").cloned(),
                });
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let reply = match method.as_str() {
                    "initialize" => {
                        let body = json!({ "jsonrpc": "2.0", "id": id, "result": { "protocolVersion": "2025-03-26", "capabilities": {} } }).to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: s-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                    "notifications/initialized" => {
                        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                    "tools/call" => {
                        let progress = json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} });
                        let result = json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [{ "type": "text", "text": "streamed" }] } });
                        let body = format!(
                            "event: message\ndata: {progress}\n\nevent: message\ndata: {result}\n\n"
                        );
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                    _ => {
                        let body = json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [{ "name": "remote" }] } }).to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                };
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (url, seen)
    }

    /// An HTTP server, in both of the shapes it may answer in: the handshake's JSON body and a
    /// call's event stream. What the definition's headers say is sent on every request, and what
    /// the handshake agreed — the session, the revision — on every request after it.
    #[test]
    fn an_http_server_is_reached_with_its_headers_its_session_and_either_answer_shape() {
        let (url, seen) = http_server();
        let hub = Hub::new(
            vec![ResolvedMcpServer {
                id: "remote".to_string(),
                transport: ResolvedTransport::Http {
                    url,
                    headers: BTreeMap::from([(
                        "Authorization".to_string(),
                        ResolvedHeader::Prefixed {
                            prefix: "Bearer ".to_string(),
                            raw: "tok".to_string(),
                        },
                    )]),
                },
            }],
            Path::new("."),
        );

        let tools = hub.list("remote", soon()).expect("list");
        assert_eq!(tools, json!([{ "name": "remote" }]));
        let called = hub
            .call("remote", "anything", &json!({}), soon())
            .expect("call");
        assert_eq!(
            called.text, "streamed",
            "the response, not the progress event ahead of it"
        );

        let seen = lock(&seen).clone();
        let methods: Vec<&str> = seen.iter().map(|s| s.method.as_str()).collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call"
            ],
            "one handshake for the two requests"
        );
        assert!(
            seen.iter()
                .all(|s| s.authorization.as_deref() == Some("Bearer tok")),
            "{seen:?}"
        );
        assert_eq!(
            seen[0].session, None,
            "there is no session before the handshake"
        );
        assert!(
            seen[1..]
                .iter()
                .all(|s| s.session.as_deref() == Some("s-1")),
            "{seen:?}"
        );
        assert!(
            seen[1..]
                .iter()
                .all(|s| s.protocol.as_deref() == Some("2025-03-26")),
            "the revision the server chose: {seen:?}"
        );
    }
}
