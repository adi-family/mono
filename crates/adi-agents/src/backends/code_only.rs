//! Code-only: an agent whose one tool is its interpreter, and whose every other tool is a
//! function inside it.
//!
//! An agent ordinarily holds a list of tools and calls them one at a time, each call a round
//! trip through the model: read a file, look at it, decide, edit it. A code-only agent
//! ([`CodeTools::only`](super::harness::tools::CodeTools)) is declared exactly one — `TsCode` or
//! `PyCode` — and writes a program instead. Everything the others would have done is a function
//! that program calls: `Read`, `Edit`, `Await`, the tools of each MCP server attached to the
//! agent. A loop over forty files is then one call rather than forty, and what the model reads
//! back is what the program chose to print rather than forty results.
//!
//! Two things make that work, and both are here:
//!
//! - [`describe`] and [`note`] — the functions are invisible unless the model is told they
//!   exist. The one tool it *is* shown says so and says where to look; the system prompt carries
//!   them, argument by argument, because a tool's description is cut where a prompt is not (see
//!   [`tools::DESCRIPTION_BUDGET`]). And each function says the same about itself when asked.
//! - [`serving`] — the door those functions go through. The interpreter is a process of its own
//!   that outlives the turn (see [`super::interpreter`]); the tools are code in whoever serves
//!   this turn's calls. So for as long as a cell runs, that process listens on a socket, the cell
//!   is told where, and each function call is one connection: a request, an answer.
//!
//! The functions are the *same* tools — [`tools::execute_inner`] runs the very table the model
//! would otherwise be declared — so nothing about what `Edit` or `Ask` does is said twice.
//!
//! # Why the door is open only while a cell runs
//!
//! A tool call needs everything a turn has: the conversation it belongs to, whether anybody is
//! watching, the MCP connections. All of that lives in the process serving the turn and ends with
//! it. A background script outlives that process by design, so it has no door and its tool
//! functions say so; what it has is the shell, which needs nobody.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write as _};
use std::os::unix::fs::DirBuilderExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::harness::tools::{self, Ctx, ToolSpec};
use super::interpreter::Language;

/// How long a connection is given to send its request. It is one line, written at once by a
/// function in the interpreter; this is only so that something else connecting to the socket and
/// saying nothing cannot hold a thread for the life of the cell.
const REQUEST_WAIT: Duration = Duration::from_secs(30);

/// The heading [`note`] opens with — also how a reader splits a composed prompt back into its
/// sections (see `runner::prompt`).
pub(crate) const NOTE_HEADING: &str = "# What your code can call";

/// `base` — the code tool's own description — for a code-only agent: told first that this is
/// all there is, what the rest became, and where that is written down.
///
/// Short, and in front, on purpose (see [`tools::DESCRIPTION_BUDGET`]). The functions
/// themselves are in [`note`], which goes into the system prompt.
pub(crate) fn describe(base: &str, language: Language) -> String {
    let names = tools::inner()
        .map(|spec| format!("`{}`", spec.name))
        .collect::<Vec<_>>()
        .join(", ");
    let called = match language {
        Language::TypeScript => "functions to `await` in this code",
        Language::Python => "functions to call in this code",
    };
    format!(
        "**This is your only tool.** Your other tools — {names}, and each MCP server as \
         `mcp.<server>` — are {called}. \"{}\" in your instructions gives their arguments, and \
         `tools()` returns them.\n\n{base}",
        NOTE_HEADING.trim_start_matches("# ")
    )
}

/// What a code-only agent's code can call, as a section of its system prompt: every other tool
/// as the function it is in `language`, argument by argument, and the MCP servers attached to
/// the agent.
///
/// `servers` are those servers' ids. Their tools are not listed: finding out what a server
/// offers means connecting to it, and this is read on every request by an agent that may never
/// use the server. A script asks, when it wants to know.
pub(crate) fn note(language: Language, servers: &[String]) -> String {
    let tool = match language {
        Language::TypeScript => "TsCode",
        Language::Python => "PyCode",
    };
    let mut text = format!(
        "{NOTE_HEADING}\n\n`{tool}` is the only tool you have. Everything else you can do is a \
         function in the code you write for it — and where anything in your instructions names \
         a tool (\"the Edit tool\", \"run it with Bash\"), it means the function of that name \
         here.\n\n"
    );
    text.push_str(match language {
        Language::TypeScript => {
            "`await` a function with one object of arguments, and it resolves to the text that \
             tool answers with. A call on a line of its own at the top level of the cell — \
             `await Read({ path: 'a.txt' });` — shows that text, as a bare `await $`…`;` shows a \
             command's output; assigned to a variable it shows nothing, and is a string for the \
             code to work on. A tool that fails rejects with `ToolError`, whose message is what \
             the tool said. Relative paths resolve from the directory the code stands in, the \
             one `$` and `fs` use. The functions are there in a cell; a background script has \
             `$`, `sh` and `share`, and no tools. `tools()` in a cell returns this list again; a bare \
             `tools();` line shows it."
        }
        Language::Python => {
            "Call a function with keyword arguments (or one dict of them), and it returns the \
             text that tool answers with. A call on a line of its own at the top level of the \
             cell — `Read(path='a.txt')` — shows that text, as a bare `sh(...)` shows a \
             command's output; assigned to a variable it shows nothing, and is a string for the \
             code to work on. A tool that fails raises `ToolError`, whose message is what the \
             tool said. Relative paths resolve from the directory the code stands in, the one \
             `sh` and `open` use. The functions are there in a cell; a background script has \
             `sh` and `share`, and no tools. `tools()` in a cell returns this list again; a bare \
             `tools()` line shows it."
        }
    });
    for spec in tools::inner() {
        let _ = write!(text, "\n\n## {}", doc(spec, language));
    }
    if !servers.is_empty() {
        let named = servers
            .iter()
            .map(|id| format!("`{}`", server_path(id)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(
            text,
            "\n\n## MCP servers\n\nThe MCP servers attached to you are objects under `mcp`: {named}. "
        );
        text.push_str(match language {
            Language::TypeScript => {
                "`await mcp.<server>.tools()` lists what one offers — each tool's `name`, \
                 `description` and `inputSchema` — and `await mcp.<server>.call(name, { … })` \
                 calls one; `mcp.<server>.<name>({ … })` is the same call where the name is an \
                 identifier. It resolves to the server's structured result when it gave one, to \
                 its text otherwise, and rejects with `ToolError` when the server says the call \
                 failed. A server is connected when you first use it. Look at `tools()` before \
                 calling anything: the names and arguments are the server's own."
            }
            Language::Python => {
                "`mcp.<server>.tools()` lists what one offers — each tool's `name`, \
                 `description` and `inputSchema` — and `mcp.<server>.call(name, { … })` calls \
                 one; `mcp.<server>.<name>(key=value)` is the same call where the name is an \
                 identifier, and `mcp[\"<server>\"]` reaches a server whose id is not one. It \
                 returns the server's structured result when it gave one, its text otherwise, \
                 and raises `ToolError` when the server says the call failed. A server is \
                 connected when you first use it. Look at `tools()` before calling anything: the \
                 names and arguments are the server's own."
            }
        });
    }
    text
}

/// One tool as code reads about it: how it is called, what it does, and each argument.
///
/// The unit [`note`] is made of — and what the function itself answers with when code asks it
/// what it is (`Edit.toString()`, `help(Edit)`), which is the first thing an agent that has
/// not read the note tries.
fn doc(spec: &ToolSpec, language: Language) -> String {
    let schema = (spec.schema)();
    let mut text = format!("`{}`\n\n{}", signature(spec, &schema, language), told(spec, language));
    fields(&mut text, &schema, "");
    text
}

/// [`doc`] for every function, by name — what a cell is handed so its functions can describe
/// themselves.
fn docs(language: Language) -> Value {
    Value::Object(
        tools::inner()
            .map(|spec| (spec.name.to_string(), Value::String(doc(spec, language))))
            .collect(),
    )
}

/// How `name` is called, for an error about how it was called: `Edit({ path, old_string, … })`.
pub(crate) fn usage(name: &str, language: Language) -> Option<String> {
    tools::inner()
        .find(|spec| spec.name == name)
        .map(|spec| signature(spec, &(spec.schema)(), language))
}

/// How a script spells the server called `id`: a property where the id can be one.
fn server_path(id: &str) -> String {
    let plain = id
        .chars()
        .enumerate()
        .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()));
    if plain && !id.is_empty() {
        format!("mcp.{id}")
    } else {
        format!("mcp[{id:?}]")
    }
}

/// What a tool says about itself, as it reads from inside code.
fn told(spec: &ToolSpec, language: Language) -> String {
    match spec.name {
        // Written for a model whose file tools resolve somewhere other than its shell stands.
        // From inside code there is one directory, and the paragraph above has said so. What
        // is added is where this tool went: each language has a spelling of its own for a
        // command, and TypeScript's is this very shell.
        "Bash" => format!(
            "{} {}",
            spec.description.replace(tools::BASH_PATHS, ""),
            match language {
                Language::TypeScript => {
                    "From code this shell is `$`: `await $`…`;` is this call without the \
                     wrapping, and `await $.background`…`;` is its `background`. Call `Bash` \
                     itself when the text it answers with is what you want."
                }
                Language::Python => {
                    "From code, `sh` is the plain way to run a command; this is here for \
                     `background`."
                }
            }
        ),
        _ => spec.description.to_string(),
    }
}

/// The call, as it is written: `Read({ path, offset?, limit? })`, `Read(path=…, offset=None,
/// limit=None)`.
fn signature(spec: &ToolSpec, schema: &Value, language: Language) -> String {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let names: Vec<&String> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().collect())
        .unwrap_or_default();
    // What must be given first, in both spellings: it is what a reader copies.
    let (must, may): (Vec<&String>, Vec<&String>) =
        names.into_iter().partition(|name| required.contains(&name.as_str()));
    match language {
        Language::TypeScript => {
            let listed: Vec<String> = must
                .iter()
                .map(|name| (*name).clone())
                .chain(may.iter().map(|name| format!("{name}?")))
                .collect();
            if listed.is_empty() {
                format!("await {}({{}})", spec.name)
            } else {
                format!("await {}({{ {} }})", spec.name, listed.join(", "))
            }
        }
        Language::Python => {
            let listed: Vec<String> = must
                .iter()
                .map(|name| format!("{name}=…"))
                .chain(may.iter().map(|name| format!("{name}=None")))
                .collect();
            format!("{}({})", spec.name, listed.join(", "))
        }
    }
}

/// One line per argument: its name, what it is, and what the tool's schema says about it — and
/// the same for what an argument is made of, indented under it.
fn fields(text: &mut String, schema: &Value, under: &str) {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for (name, property) in properties {
        let optional = if required.contains(&name.as_str()) {
            ""
        } else {
            ", optional"
        };
        let _ = write!(text, "\n- `{under}{name}` ({}{optional})", kind(property));
        if let Some(description) = property.get("description").and_then(Value::as_str) {
            let _ = write!(text, ": {description}");
        }
        // An object's own fields, or those of the objects an array holds.
        let (inside, path) = match property.get("items") {
            Some(items) => (items, format!("{under}{name}[].")),
            None => (property, format!("{under}{name}.")),
        };
        fields(text, inside, &path);
    }
}

/// A schema's type, in the words the schema itself uses.
fn kind(schema: &Value) -> String {
    let named = schema.get("type").and_then(Value::as_str).unwrap_or("any");
    match (named, schema.get("items")) {
        ("array", Some(items)) => format!("array of {}", kind(items)),
        _ => named.to_string(),
    }
}

/// The socket a cell's tool functions connect to, in a directory of its own.
struct Door {
    dir: PathBuf,
    path: PathBuf,
    listener: UnixListener,
}

impl Door {
    fn open() -> Result<Self, String> {
        // A socket path has about a hundred bytes to fit in, and a temp directory that came from
        // `$TMPDIR` can be most of that on its own — so a long one is traded for `/tmp`. The
        // directory is made 0700, which is the whole of the access control: whoever can enter it
        // is this user.
        let name = format!("adi-door-{}", uuid::Uuid::new_v4().simple());
        let mut dir = std::env::temp_dir().join(&name);
        if dir.as_os_str().len() > 80 {
            dir = Path::new("/tmp").join(&name);
        }
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
        let path = dir.join("d.sock");
        let listener = UnixListener::bind(&path).map_err(|e| {
            let _ = std::fs::remove_dir(&dir);
            format!("couldn't listen on {}: {e}", path.display())
        })?;
        Ok(Self {
            dir,
            path,
            listener,
        })
    }
}

impl Drop for Door {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// Run a cell with the door open: `run` is handed what the interpreter is to be told about its
/// tools, and for as long as it takes, each function the cell calls is answered here.
///
/// `timeout_ms` is the cell's own deadline. A tool the cell calls cannot outlast it — a command
/// started through `Bash` is given what is left of it, and so is a call to an MCP server — so
/// that a cell stopped at its deadline is not kept waiting on something it has stopped waiting
/// for.
///
/// # Errors
/// A message for the model when the door cannot be opened at all.
pub(crate) fn serving<T>(
    ctx: &Ctx<'_>,
    timeout_ms: u64,
    run: impl FnOnce(&Value) -> T,
) -> Result<T, String> {
    let door = Door::open()?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let offered = json!({
        "socket": door.path.to_string_lossy(),
        "names": tools::inner().map(|spec| spec.name).collect::<Vec<_>>(),
        "servers": ctx.mcp.ids(),
        // What each function says when code asks it what it is.
        "docs": docs(ctx.code.language()),
    });
    let closing = AtomicBool::new(false);

    // Never blocked in `accept`: this thread has to be able to end when the cell does, and the
    // scope below does not return until it has.
    door.listener
        .set_nonblocking(true)
        .map_err(|e| format!("couldn't listen on {}: {e}", door.path.display()))?;

    let ran = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !closing.load(Ordering::SeqCst) {
                if !readable(&door.listener, POLL) {
                    continue;
                }
                // Nothing there after all — a caller that hung up between the two calls.
                let Ok((conn, _)) = door.listener.accept() else {
                    continue;
                };
                if closing.load(Ordering::SeqCst) {
                    break;
                }
                // What is accepted from a non-blocking listener is non-blocking too on some
                // systems, and the conversation on it is a plain blocking read and write.
                if conn.set_nonblocking(false).is_err() {
                    continue;
                }
                // A thread per call: a cell is free to start several at once, and a slow MCP
                // server must not stand in front of a `Read`.
                scope.spawn(move || answer_on(conn, ctx, deadline));
            }
        });
        let ran = run(&offered);
        // Connecting is what wakes the listener at once to see the flag. Should that fail, it
        // sees it within `POLL` anyway.
        closing.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&door.path);
        ran
    });
    Ok(ran)
}

/// How long the door waits for a connection before looking again at whether it is closing. A
/// backstop only — closing wakes it by connecting — so it is how long a cell's end could be held
/// up if that wake were ever lost, and nothing a call waits on.
const POLL: Duration = Duration::from_millis(100);

/// Wait up to `wait` for a connection to be pending on `listener`.
fn readable(listener: &UnixListener, wait: Duration) -> bool {
    use std::os::fd::AsRawFd as _;

    let mut pending = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = i32::try_from(wait.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: `poll(2)` reads and writes the one `pollfd` it is given, which lives on this stack
    // frame for the whole call, and the descriptor is the listener's, borrowed for as long.
    #[allow(unsafe_code)]
    let ready = unsafe { libc::poll(&raw mut pending, 1, millis) };
    if ready < 0 {
        // Interrupted, most likely. Not spun on: whatever it is, it is looked at again shortly.
        std::thread::sleep(Duration::from_millis(5));
    }
    ready > 0
}

/// One connection: a request line in, an answer line out.
fn answer_on(mut conn: UnixStream, ctx: &Ctx<'_>, deadline: Instant) {
    let _ = conn.set_read_timeout(Some(REQUEST_WAIT));
    let mut line = String::new();
    let read = conn
        .try_clone()
        .and_then(|reader| BufReader::new(reader).read_line(&mut line));
    if !matches!(read, Ok(n) if n > 0) {
        return;
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(message) => answer(&message, ctx, deadline),
        Err(e) => failed(&format!("that was not a tool call: {e}")),
    };
    // Whoever asked may have stopped waiting — a cell past its deadline hangs up. Then there is
    // nobody to tell, and nothing to do about it.
    let _ = writeln!(conn, "{reply}").and_then(|()| conn.flush());
}

fn failed(text: &str) -> Value {
    json!({ "ok": false, "text": text })
}

/// Run what one request asks for: a tool of the platform's, or one of an MCP server's.
fn answer(message: &Value, ctx: &Ctx<'_>, deadline: Instant) -> Value {
    let input = message
        .get("input")
        .filter(|input| !input.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let named = |key: &str| message.get(key).and_then(Value::as_str);

    if let Some(server) = named("mcp") {
        if message.get("list").and_then(Value::as_bool) == Some(true) {
            return match ctx.mcp.list(server, deadline) {
                Ok(listed) => json!({ "ok": true, "value": listed }),
                Err(e) => failed(&e),
            };
        }
        let Some(tool) = named("tool") else {
            return failed("which tool of the server's to call was not said");
        };
        return match ctx.mcp.call(server, tool, &input, deadline) {
            Ok(outcome) if outcome.is_error => failed(if outcome.text.trim().is_empty() {
                "the server said the call failed, and nothing more"
            } else {
                &outcome.text
            }),
            Ok(outcome) => json!({ "ok": true, "text": outcome.text, "value": outcome.value }),
            Err(e) => failed(&e),
        };
    }

    let Some(name) = named("tool") else {
        return failed("which tool to call was not said");
    };
    // Where the code stands, so that a relative path means to `Read` what it means to the `fs`
    // call on the line above it. A directory that has since gone falls back to the run's own.
    let stands = named("cwd")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(|| ctx.cwd.to_path_buf());
    let inner = Ctx {
        cwd: &stands,
        shell: ctx.shell.clone(),
        agent: ctx.agent,
        conv: ctx.conv,
        awaits: ctx.awaits.clone(),
        sessions: ctx.sessions.clone(),
        agent_dir: ctx.agent_dir,
        unattended: ctx.unattended,
        code: ctx.code,
        mcp: ctx.mcp.clone(),
    };
    match tools::execute_inner(name, &input, &inner, deadline) {
        Ok(text) => json!({ "ok": true, "text": text }),
        Err(text) => failed(&text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one tool a code-only agent is shown says, before anything else, that it is the only
    /// one, what the others became and where they are written down — in few enough words that
    /// the description it leads is still read whole.
    #[test]
    fn the_description_leads_with_what_became_of_the_other_tools() {
        let ts = describe("BASE.", Language::TypeScript);
        assert!(ts.starts_with("**This is your only tool.** Your other tools — `Read`, `Write`, `Edit`, `Bash`, `Glob`, `Grep`, `Await`, `Ask`, `Report`, and each MCP server as `mcp.<server>` — are functions to `await` in this code."), "{ts}");
        assert!(ts.contains("\"What your code can call\" in your instructions"), "{ts}");
        assert!(ts.ends_with("\n\nBASE."), "{ts}");
        assert!(ts.chars().count() < "BASE.".len() + 400, "a lead, not a list: {ts}");

        let py = describe("BASE.", Language::Python);
        assert!(py.contains("are functions to call in this code"), "{py}");
        assert!(!py.contains("await"), "{py}");
    }

    /// The section of the system prompt is the only place a code-only agent learns how to call
    /// what it has, so each tool has to be in it — spelled the way the language calls it — with
    /// its arguments, including the ones nested inside another.
    #[test]
    fn the_note_carries_every_other_tool_in_the_language_it_is_called_from() {
        let ts = note(Language::TypeScript, &[]);
        assert!(ts.starts_with("# What your code can call\n\n`TsCode` is the only tool you have."), "{ts}");
        assert!(ts.contains("it means the function of that name here"), "{ts}");
        for spec in tools::inner() {
            assert!(ts.contains(&format!("\n\n## `await {}(", spec.name)), "{} is missing: {ts}", spec.name);
        }
        assert!(ts.contains("## `await Read({ path, limit?, offset? })`\n\nRead a file from the filesystem."), "{ts}");
        assert!(ts.contains("\n- `path` (string): File to read."), "{ts}");
        assert!(ts.contains("\n- `offset` (integer, optional):"), "{ts}");
        assert!(
            ts.contains("\n- `questions[].options[].label` (string):"),
            "what an argument is made of is described too: {ts}"
        );
        assert!(ts.contains("\n- `events` (array of string, optional):"), "{ts}");
        assert!(!ts.contains("PyCode(") && !ts.contains("TsCode("), "the code tools are not functions: {ts}");
        assert!(!ts.contains("mcp."), "no servers, so nothing about them: {ts}");

        let py = note(Language::Python, &[]);
        assert!(py.starts_with("# What your code can call\n\n`PyCode` is the only tool you have."), "{py}");
        assert!(py.contains("## `Read(path=…, limit=None, offset=None)`"), "{py}");
        assert!(py.contains("## `Await(after_seconds=None,"), "{py}");
        assert!(!py.contains("await Read"), "{py}");
    }

    /// From inside code there is one directory, so the sentence telling a model its file tools
    /// resolve somewhere else than its shell is not said — and the shell tool says where it
    /// went, which is a different place in each language.
    #[test]
    fn the_shell_tool_is_described_for_what_it_is_from_inside_code() {
        let ts = note(Language::TypeScript, &[]);
        assert!(!ts.contains("Only the shell moves"), "{ts}");
        assert!(ts.contains("From code this shell is `$`"), "{ts}");
        assert!(ts.contains("`await $.background`…`;` is its `background`"), "{ts}");
        assert!(ts.contains("It is one shell across the whole conversation"), "the rest of it stands: {ts}");

        // Python has no template to write a command in, so there the function is the way.
        let py = note(Language::Python, &[]);
        assert!(!py.contains("Only the shell moves"), "{py}");
        assert!(py.contains("this is here for `background`"), "{py}");
        assert!(!py.contains("$.background"), "{py}");
    }

    /// Servers are named, not listed: a script asks one what it offers.
    #[test]
    fn attached_servers_are_named_with_how_to_ask_what_they_offer() {
        let servers = ["n8n".to_string(), "my-server".to_string()];
        let ts = note(Language::TypeScript, &servers);
        assert!(ts.contains("## MCP servers\n\nThe MCP servers attached to you are objects under `mcp`: `mcp.n8n`, `mcp[\"my-server\"]`."), "{ts}");
        assert!(ts.contains("await mcp.<server>.tools()"), "{ts}");
        let py = note(Language::Python, &servers);
        assert!(py.contains("`mcp.<server>.call(name, { … })`"), "{py}");
        assert!(!py.contains("await mcp"), "{py}");
    }

    /// What a function says about itself is its entry in the note, and how it is called is the
    /// first line of that.
    #[test]
    fn a_function_is_documented_once_and_says_the_same_everywhere() {
        let said = docs(Language::TypeScript);
        let edit = said["Edit"].as_str().expect("a doc");
        assert!(note(Language::TypeScript, &[]).contains(&format!("## {edit}")), "{edit}");
        assert_eq!(
            usage("Edit", Language::TypeScript).as_deref(),
            Some("await Edit({ new_string, old_string, path, replace_all? })")
        );
        assert!(edit.starts_with("`await Edit({ new_string, old_string, path, replace_all? })`\n\n"), "{edit}");
        assert_eq!(usage("TsCode", Language::TypeScript), None, "not a function");
    }
}
