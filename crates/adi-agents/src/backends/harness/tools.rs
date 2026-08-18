//! The tools an `harness:adi` turn can call, and the code that runs them.
//!
//! These are the file and shell tools every coding agent has — `Read`, `Write`, `Edit`, `Bash`,
//! `Glob`, `Grep` — declared once here in a provider-neutral shape and translated into each
//! provider's own function-calling dialect by [`super::adi_loop`]. The names match the ones the
//! Claude backends use, because an agent's prompt (and the person writing it) shouldn't have to
//! learn a second vocabulary to move between backends.
//!
//! **There is no path jail, deliberately.** `Bash` runs a shell, so confining `Read` to the run's
//! working directory would stop nothing and only mislead whoever read the code. What the tools do
//! instead is resolve every relative path against the run's own directory — the one `workspace`
//! picked and the child was spawned into — so an agent that says `src/lib.rs` means the file its
//! run is about, and an agent that means something else says so in full.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::backends::jobs;
use crate::backends::shell::Shell;
use crate::awaits::{self, Awaits, Request};
use crate::store::{AskRequest, Choice, MAX_QUESTIONS, SessionStore};

/// A tool as the model sees it: a name, a sentence about when to reach for it, and the JSON Schema
/// of its arguments. Providers disagree only about where these three go on the wire.
pub(crate) struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The JSON Schema `object` describing this tool's arguments.
    pub schema: fn() -> Value,
}

/// What a tool call knows about the turn making it.
///
/// Most tools need only [`cwd`](Self::cwd) — they act on files and processes, and the run's own
/// directory is the whole of their world. [`Await`](TOOLS) needs the other two: a wake is delivered
/// by replying into *this* conversation, so registering one means naming it.
pub(crate) struct Ctx<'a> {
    /// The directory the run is about; every relative path resolves against it.
    pub cwd: &'a Path,
    /// The conversation's shell — where its last command left off, and what it exported. Held here
    /// because it belongs to the conversation rather than to any one call (see [`super::shell`]).
    pub shell: Shell,
    /// The agent this turn belongs to.
    pub agent: &'a str,
    /// The conversation this turn belongs to — where a wake is delivered.
    pub conv: &'a str,
    /// Where a registered wake is written. Held here rather than opened at the call site so a test
    /// can point it at a scratch store without touching the environment every other test reads.
    pub awaits: Awaits,
    /// The session store — where an [`Ask`](TOOLS) writes the question that outlives this turn.
    /// Held for the same reason as `awaits`: a test points it somewhere harmless.
    pub sessions: SessionStore,
    /// This agent's session directory — where a background job files its log and its status, in the
    /// session's own `<id>.*` namespace beside the shell's own sidecars.
    pub agent_dir: &'a Path,
    /// Whether this agent runs with nobody watching (see
    /// [`AgentManifest::unattended`](crate::AgentManifest::unattended)). The one thing it changes
    /// is that `Ask` refuses.
    pub unattended: bool,
}

/// What `Bash` is, told in the terms that decide how a command gets written: the shell is the
/// conversation's, not the call's. Two texts because that is only true where [`super::shell`] can
/// keep it — a model on Windows told it could name a path once would name it into nothing.
#[cfg(unix)]
const BASH: &str = "Run a shell command and return its output (stdout and stderr together, with \
                    the exit status when it is not zero). It is one shell across the whole \
                    conversation: `cd` moves it until you move it again, and an **exported** \
                    variable is still set on your next call. So name a long path once — `export \
                    FE=/long/path/to/a/checkout` — and use `$FE` from then on, rather than writing \
                    it out on command after command. A bare `FE=…` is not exported and does not \
                    carry. Only the shell moves: Read, Write, Edit, Glob and Grep go on resolving \
                    relative paths against the run's own directory. Set \
                    `background` for a command that will outlast the turn: it is started and you \
                    get a job id back straight away, then **end your turn** — you are woken when it \
                    exits, carrying its exit status and the tail of its output. That is how to run \
                    a build, a test suite, or another agent without sitting inside the call waiting \
                    for it. Do not poll a job you started; being woken is the whole point.";
#[cfg(windows)]
const BASH: &str = "Run a shell command in the working directory and return its output (stdout \
                    and stderr together, with the exit status when it is not zero). Each call is \
                    its own shell, so a `cd` or a variable you set is gone by the next one — use \
                    full paths.";

/// Everything a turn may call. Order is the order they're advertised, which is the order a model
/// tends to consider them in: look before you write, and shell out only when nothing else fits.
pub(crate) const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "Read",
        description: "Read a file from the filesystem. Prefer this over `cat` in Bash: it returns \
                      the text directly and says so when a file was too large to return whole.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to read. Relative paths resolve against the working directory." },
                    "offset": { "type": "integer", "description": "First line to return (1-based). Omit to start at the beginning." },
                    "limit": { "type": "integer", "description": "How many lines to return. Omit for as much as fits." },
                },
                "required": ["path"],
            })
        },
    },
    ToolSpec {
        name: "Write",
        description: "Write a file, creating it and any missing parent directories, and replacing \
                      it whole if it already exists. To change part of a file, use Edit instead.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to write." },
                    "content": { "type": "string", "description": "The file's complete new contents." },
                },
                "required": ["path", "content"],
            })
        },
    },
    ToolSpec {
        name: "Edit",
        description: "Replace an exact string in a file. `old_string` must appear exactly once \
                      unless `replace_all` is true — include enough surrounding text to make it \
                      unique. Read the file first; an edit against text you have not seen fails.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to edit." },
                    "old_string": { "type": "string", "description": "Text to replace, copied exactly from the file." },
                    "new_string": { "type": "string", "description": "Text to put in its place." },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence instead of requiring exactly one." },
                },
                "required": ["path", "old_string", "new_string"],
            })
        },
    },
    ToolSpec {
        name: "Bash",
        description: BASH,
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command line to run." },
                    "timeout_ms": { "type": "integer", "description": "Give up after this long. Defaults to 120000. Ignored when `background` is set — a job has no deadline." },
                    "background": { "type": "boolean", "description": "Start the command and return immediately with a job id instead of waiting for it. Use it for anything that outlasts a couple of minutes — a build, a test suite, a deploy, another agent. You are woken when it exits, with its status and the tail of its log." },
                },
                "required": ["command"],
            })
        },
    },
    ToolSpec {
        name: "Glob",
        description: "List files matching a glob pattern (`*`, `?`, and `**` for any depth), \
                      newest first. Use it to find files by name when you don't know where they are.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob to match against paths, e.g. `**/*.rs`." },
                    "path": { "type": "string", "description": "Directory to search. Defaults to the working directory." },
                },
                "required": ["pattern"],
            })
        },
    },
    ToolSpec {
        name: "Grep",
        description: "Search file contents with a regular expression and return matching lines as \
                      `path:line: text`. Use it to find code by what it says rather than its name.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Rust/PCRE-style regular expression." },
                    "path": { "type": "string", "description": "File or directory to search. Defaults to the working directory." },
                    "glob": { "type": "string", "description": "Only search paths matching this glob, e.g. `**/*.toml`." },
                    "case_insensitive": { "type": "boolean", "description": "Ignore case while matching." },
                },
                "required": ["pattern"],
            })
        },
    },
    ToolSpec {
        name: "Await",
        description:
            "Ask to be woken later, then carry on and finish this turn. When one of `events` is \
             published — or the timer comes due — your `check` command decides whether it is really \
             the moment: exit 0 wakes you, anything else leaves the await waiting. Without a check, \
             the first event or deadline wakes you. **A check needs no events at all**: \
             `every_seconds` with a `check` is a script of yours running on a schedule, waking you \
             only when it says so — the way to wait on anything the platform publishes no event for \
             (a build finishing, a file appearing, an endpoint coming up). Waking delivers a new \
             message into this same \
             conversation carrying your `note`, what happened, and what the check printed, and you \
             continue with the whole transcript in front of you. A wake fires once — register \
             another if you still need one. Be specific with patterns: `adi.agents.**` wakes you on \
             your own runs — and `when` narrows an event to the one you mean, which is how you wait \
             on *this* run finishing rather than on any run finishing.\n\nSome things register an \
             await for you: starting an agent hands you back the id of the wake that will report it, \
             so you need not ask for one. **Two verbs act on a wake you already hold.** `ignore` \
             drops it — reach for that the moment you decide you don't need the update, because you \
             may hold only a few at a time and one you are not going to read still takes a slot. \
             `update` changes a pending await in place instead of registering a second one: name it \
             and give only the fields you want different — a longer deadline, a wider check, a note \
             that says why rather than what. Changing beats replacing, which leaves a gap the thing \
             you are waiting on can finish inside.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "note": { "type": "string", "description": "What to tell yourself when you wake: why you asked and what to do next. Handed back verbatim, and it is all you get — the turn that wrote it is over." },
                    "events": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Platform event patterns to wake on, e.g. `adi.tasks.created`, `adi.tasks.*` (one segment), `adi.**` (the tail).",
                    },
                    "after_seconds": { "type": "integer", "description": "Wake this many seconds from now." },
                    "every_seconds": { "type": "integer", "description": "Run the check this often, starting one interval from now, until it passes. With no `events`, this is the whole await: your script on a schedule." },
                    "check": { "type": "string", "description": "A shell command deciding whether it is really the moment. Exit 0 wakes you; anything else means not yet. Runs in this conversation's directory with $ADI_CAUSE, and $ADI_EVENT/$ADI_PAYLOAD when an event woke it. What it prints reaches you with the wake — so make it report what it found, not just succeed or fail." },
                    "expires_in_seconds": { "type": "integer", "description": "Give up after this long and wake you anyway, saying it lapsed." },
                    "when": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Payload fields a matching event must all carry, e.g. `{\"run_id\": \"…\"}`. Without it you wake on every event of that name, including other people's.",
                    },
                    "update": { "type": "string", "description": "The id of a pending await to change instead of registering a new one. Only the fields you also give are changed; everything else about it stays. An empty `check` removes its check." },
                    "ignore": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Ids of pending awaits to drop. Use it on a wake you were handed and don't want. May be the only argument you give.",
                    },
                },
                "required": [],
            })
        },
    },
    ToolSpec {
        name: "Ask",
        description:
            "Put a decision to the person you are working for, then **end your turn**. You are not \
             waiting here: the question is written down, this conversation is marked as blocked on \
             them, and their answer arrives as a new message with your whole transcript still in \
             front of you. Nothing is held open in the meantime.\n\nReach for this when the work \
             genuinely forks on something only they can settle — which of two designs to build, \
             whether to touch production, which of three ambiguous readings of the request is the \
             real one. Do **not** reach for it to confirm something you can determine yourself, to \
             ask permission for what you were already asked to do, or to report progress. A \
             judgement call you make and state plainly is worth more than a question that stops the \
             work, so if you can proceed under a stated assumption, do that instead.\n\nAsk for \
             everything you need in **one** call — up to four questions. Every answer costs a full \
             turn, so four questions asked one at a time cost four, and the person answering sees \
             the whole decision at once instead of being led through it a click at a time. Give \
             `options` wherever you can: an answer someone taps is an answer you get, and one they \
             have to compose is one you wait for.\n\nNever ask for a password, an API key, or any \
             other secret — the answer is stored in this transcript in the clear and replayed to \
             the model on every later turn. Ask them to store it (`adi-mono secrets set NAME`) and \
             tell you the name.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "note": { "type": "string", "description": "What you were doing when you stopped to ask, in a sentence or two. Shown above the questions — a question read without context gets answered wrongly by someone who would have got it right." },
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_QUESTIONS,
                        "description": "Everything you need decided, asked together.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "header": { "type": "string", "description": "Two or three words naming the decision — `Auth method`, `Rollout`. It is the chip shown beside the question, and what a notification says." },
                                "question": { "type": "string", "description": "The question, as you would put it to a colleague." },
                                "options": {
                                    "type": "array",
                                    "description": "The answers worth offering as one tap. Leave empty only when the answer really is free-form.",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": { "type": "string", "description": "What the button says. Short — a few words." },
                                            "description": { "type": "string", "description": "What choosing it implies. This is where the trade-off goes, and it is what turns picking a word into making a decision." },
                                        },
                                        "required": ["label"],
                                    },
                                },
                                "multi_select": { "type": "boolean", "description": "Whether more than one option may be chosen. Default false." },
                            },
                            "required": ["question"],
                        },
                    },
                    "after_seconds": { "type": "integer", "description": "Stop waiting after this long and take `defaults` instead. Use it whenever the work should not stall on silence; without it the question waits indefinitely, which is right only when you know somebody is watching." },
                    "defaults": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "What to assume per question when `after_seconds` passes, in the same order. Required with `after_seconds` — a deadline is a statement about what happens when nobody answers, so it is only a deadline if you say what you will assume.",
                    },
                },
                "required": ["questions"],
            })
        },
    },
];

/// How much of a tool's output goes back to the model. A turn replays its whole transcript on every
/// round, so an unbounded `Read` of a large file is paid for again on each one — the cap is what
/// keeps one careless call from crowding out the rest of the conversation.
const MAX_OUTPUT: usize = 32_000;
/// Directories never worth walking: build output and version-control internals, which are large,
/// uninteresting, and would drown a `Glob` in noise.
const SKIP_DIRS: &[&str] = &["target", "node_modules", ".git", "dist", ".build", "__pycache__"];
/// How long a `Bash` command may run before it is killed, when the call names no timeout.
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// One tool as a model is told about it: the three things every provider's dialect carries, in a
/// shape that is nobody's dialect.
///
/// The owned twin of [`ToolSpec`], which is `'static` data this crate keeps in a table. This is
/// what leaves the crate — a screen that lets a person take the model's seat has to show them the
/// tools the model was actually declared, and it cannot be handed a `&'static [ToolSpec]` full of
/// function pointers.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolDeclaration {
    /// The name a call writes.
    pub name: String,
    /// The sentence the model is given about when to reach for it.
    pub description: String,
    /// The JSON Schema `object` describing the call's arguments.
    pub schema: Value,
}

/// Every tool an `harness:adi` turn can call, exactly as declared to the model.
///
/// Read off the same [`TOOLS`] table [`super::adi_loop`] builds its provider payloads from, so a
/// tool added there appears here without anybody remembering to. Anything that renders this list
/// for a person is then showing the inventory the model has rather than a second copy of it that
/// can fall behind.
#[must_use]
pub fn declarations() -> Vec<ToolDeclaration> {
    TOOLS
        .iter()
        .map(|t| ToolDeclaration {
            name: t.name.to_string(),
            description: t.description.to_string(),
            schema: (t.schema)(),
        })
        .collect()
}

impl<'a> Ctx<'a> {
    /// The context one conversation's tool calls run in.
    ///
    /// Built here rather than at each call site because the pieces are not interchangeable: the
    /// shell is keyed by the *conversation*, so a second construction that keyed it by the turn
    /// would silently give a run a fresh shell — no `cd`, no exports — and the tools would keep
    /// working while quietly meaning something else. The adi loop and the simulator both come
    /// through here, which is what makes a call made by a person the same call the model makes.
    pub(crate) fn for_conversation(
        agent: &'a str,
        unattended: bool,
        cwd: &'a Path,
        conv: &'a str,
        agent_dir: &'a Path,
        sessions: SessionStore,
    ) -> Self {
        Self {
            cwd,
            shell: Shell::new(agent_dir, conv),
            agent,
            conv,
            awaits: awaits::Awaits::open(),
            sessions,
            agent_dir,
            unattended,
        }
    }
}

/// Run one tool call in `cwd`.
///
/// The error half of the result is not a failure of the loop — it is the tool's *answer*, handed
/// back to the model as a failed result so it can correct itself and try again, which is why every
/// message here is written to be read by the model rather than by a log reader.
pub(crate) fn execute(
    name: &str,
    input: &Value,
    ctx: &Ctx<'_>,
) -> std::result::Result<String, String> {
    match name {
        "Read" => read(input, ctx.cwd),
        "Write" => write(input, ctx.cwd),
        "Edit" => edit(input, ctx.cwd),
        "Bash" => bash(input, ctx),
        "Glob" => glob(input, ctx.cwd),
        "Grep" => grep(input, ctx.cwd),
        "Await" => await_wake(input, ctx),
        "Ask" => ask(input, ctx),
        other => Err(format!(
            "no tool named {other} — the tools you have are: {}",
            TOOLS
                .iter()
                .map(|t| t.name)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

// ---- the tools ---------------------------------------------------------------------

fn read(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let path = resolve(arg_str(input, "path")?, cwd);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;

    // Saturating rather than casting: a model that asks for line 2^40 of a file gets the end of
    // it, not a number that wrapped into something else entirely.
    let offset = usize::try_from(arg_u64(input, "offset").unwrap_or(1).max(1)).unwrap_or(usize::MAX);
    let limit = usize::try_from(arg_u64(input, "limit").unwrap_or(u64::MAX)).unwrap_or(usize::MAX);
    let lines: Vec<&str> = text.lines().skip(offset - 1).take(limit).collect();
    if lines.is_empty() {
        return Ok(format!(
            "{} has no lines at offset {offset} (the file has {}).",
            path.display(),
            text.lines().count()
        ));
    }
    Ok(truncate(&lines.join("\n")))
}

fn write(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let path = resolve(arg_str(input, "path")?, cwd);
    let content = arg_str(input, "content")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("couldn't create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    Ok(format!(
        "wrote {} ({} bytes)",
        path.display(),
        content.len()
    ))
}

fn edit(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let path = resolve(arg_str(input, "path")?, cwd);
    let old = arg_str(input, "old_string")?;
    let new = arg_str(input, "new_string")?;
    let all = input
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if old == new {
        return Err("old_string and new_string are identical — nothing to change".to_string());
    }

    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    let hits = text.matches(old).count();
    let edited = match (hits, all) {
        (0, _) => {
            return Err(format!(
                "old_string is not in {} — read the file and copy the text exactly, including \
                 whitespace",
                path.display()
            ));
        }
        (n, false) if n > 1 => {
            return Err(format!(
                "old_string appears {n} times in {} — add surrounding text until it is unique, or \
                 pass replace_all",
                path.display()
            ));
        }
        (_, true) => text.replace(old, new),
        (_, false) => text.replacen(old, new, 1),
    };
    std::fs::write(&path, &edited)
        .map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    Ok(format!(
        "edited {} ({} replacement{})",
        path.display(),
        if all { hits } else { 1 },
        if all && hits != 1 { "s" } else { "" }
    ))
}

fn bash(input: &Value, ctx: &Ctx<'_>) -> std::result::Result<String, String> {
    let command = arg_str(input, "command")?;
    if input.get("background").and_then(Value::as_bool).unwrap_or(false) {
        return background(command, ctx);
    }
    let (cwd, shell) = (ctx.cwd, &ctx.shell);
    let timeout = arg_u64(input, "timeout_ms").unwrap_or(DEFAULT_TIMEOUT_MS);
    let start = shell.start_dir(cwd);
    let script = shell.script(command);

    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(&script);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&script);
        c
    };
    cmd.current_dir(&start);

    let output = wait_with_timeout(cmd, timeout)?;
    let moved = shell
        .moved_from(&start)
        .map(|ended| format!("\n(the shell is now in {})", ended.display()));
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    if !output.status.success() {
        // A non-zero exit is information, not a loop failure: the model usually wants to read the
        // output and try something else, so it comes back as a result with the status attached.
        let code = output
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |c| c.to_string());
        return Ok(with_note(&truncate(&format!("(exit {code})\n{text}")), moved));
    }
    let body = truncate(if text.trim().is_empty() {
        "(no output)"
    } else {
        &text
    });
    Ok(with_note(&body, moved))
}

/// Start a command as a background job and register the wake that will report it.
///
/// The order matters and is deliberate. The job is started *first*, then the wake is registered: the
/// other way round leaves an await watching a job that failed to start. So a wake that cannot be
/// registered is reported as exactly that — the job is already running, and killing work the model
/// asked for because a bookkeeping record didn't fit would be the worse of the two failures. The log
/// path travels either way, which is what makes that recoverable rather than merely honest.
fn background(command: &str, ctx: &Ctx<'_>) -> std::result::Result<String, String> {
    let start = ctx.shell.start_dir(ctx.cwd);
    let job = jobs::start(ctx.agent_dir, ctx.conv, &ctx.shell, &start, command)
        .map_err(|e| format!("couldn't start the job: {e}"))?;

    let request = Request {
        note: format!("You started this in the background:\n\n{command}"),
        every_seconds: Some(jobs::LOOK_EVERY_SECONDS),
        check: Some(jobs::done_check(&job)),
        cwd: start.display().to_string(),
        ..Request::default()
    };
    let log = job.log.display();
    match awaits::register(&ctx.awaits, ctx.agent, ctx.conv, &request) {
        Ok(_) => Ok(format!(
            "Started {} in the background.\n  log: {log}\n\nIt is running now and this call is \
             done. Finish your turn — you will be woken with its exit status and the tail of that \
             log when it ends. Don't poll it.",
            job.id
        )),
        // The job is real and running; only the wake is missing. Say which is which.
        Err(e) => Ok(format!(
            "Started {} in the background, but nothing will wake you when it ends: {e}\n  log: \
             {log}\n\nRead that log to find out how it went.",
            job.id
        )),
    }
}

/// `body` with a note behind it. Appended after truncation, never before it: a note about where the
/// shell now is is worthless if a long command's output is what cut it off.
fn with_note(body: &str, note: Option<String>) -> String {
    match note {
        Some(note) => format!("{body}{note}"),
        None => body.to_string(),
    }
}

fn glob(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let pattern = arg_str(input, "pattern")?;
    let root = match input.get("path").and_then(Value::as_str) {
        Some(p) if !p.trim().is_empty() => resolve(p, cwd),
        _ => cwd.to_path_buf(),
    };

    let mut hits: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    walk(&root, &mut |path| {
        let Ok(rel) = path.strip_prefix(&root) else {
            return;
        };
        if glob_match(pattern, &rel.to_string_lossy()) {
            let at = path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            hits.push((at, path.to_path_buf()));
        }
    });
    if hits.is_empty() {
        return Ok(format!("no files match {pattern} under {}", root.display()));
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    let listed: Vec<String> = hits
        .iter()
        .map(|(_, p)| p.display().to_string())
        .collect();
    Ok(truncate(&listed.join("\n")))
}

fn grep(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let pattern = arg_str(input, "pattern")?;
    let insensitive = input
        .get("case_insensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(insensitive)
        .build()
        .map_err(|e| format!("{pattern} is not a valid regular expression: {e}"))?;
    let only = input
        .get("glob")
        .and_then(Value::as_str)
        .filter(|g| !g.trim().is_empty());
    let root = match input.get("path").and_then(Value::as_str) {
        Some(p) if !p.trim().is_empty() => resolve(p, cwd),
        _ => cwd.to_path_buf(),
    };

    let mut out = String::new();
    let mut search = |path: &Path, rel: &str| {
        if let Some(g) = only
            && !glob_match(g, rel)
        {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return; // a binary or unreadable file is simply not a match
        };
        for (n, line) in text.lines().enumerate() {
            if re.is_match(line) && out.len() < MAX_OUTPUT {
                let _ = writeln!(out, "{}:{}: {}", path.display(), n + 1, line.trim());
            }
        }
    };
    if root.is_file() {
        let rel = root.file_name().map(|n| n.to_string_lossy().into_owned());
        search(&root, rel.as_deref().unwrap_or(""));
    } else {
        walk(&root, &mut |path| {
            let rel = path
                .strip_prefix(&root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_default();
            search(path, &rel);
        });
    }
    if out.trim().is_empty() {
        return Ok(format!("no matches for {pattern} under {}", root.display()));
    }
    Ok(truncate(&out))
}

/// Register a wake for this conversation. The turn is not interrupted: the model gets its
/// confirmation back as a tool result and goes on to finish its answer, which is what makes
/// "subscribe, then wrap up what I was doing" expressible at all.
fn await_wake(input: &Value, ctx: &Ctx<'_>) -> std::result::Result<String, String> {
    let dropping = string_list(input, "ignore");
    if !dropping.is_empty() {
        return drop_awaits(&dropping, ctx);
    }
    if let Some(id) = arg_id(input, "update") {
        return change_await(id, input, ctx);
    }
    let req = Request {
        note: arg_str(input, "note")?.to_string(),
        events: string_list(input, "events"),
        when: string_map(input, "when")?,
        after_seconds: arg_u64(input, "after_seconds"),
        every_seconds: arg_u64(input, "every_seconds"),
        check: input
            .get("check")
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_in_seconds: arg_u64(input, "expires_in_seconds"),
        cwd: ctx.cwd.display().to_string(),
    };
    let registered =
        awaits::register(&ctx.awaits, ctx.agent, ctx.conv, &req).map_err(|e| e.to_string())?;
    Ok(format!(
        "registered await {} — waking {}. Finish this turn; you will be woken with your note.",
        registered.id,
        registered.describe()
    ))
}

/// Drop wakes this conversation is holding — the answer to a wake it never asked for.
///
/// Every id is attempted and every outcome is reported, rather than stopping at the first failure.
/// A model dropping three ids has usually just decided it wants none of them, and leaving two of the
/// three registered because the first was already spent is not what it asked for.
fn drop_awaits(ids: &[String], ctx: &Ctx<'_>) -> std::result::Result<String, String> {
    let mut lines = Vec::new();
    for id in ids {
        lines.push(match awaits::ignore(&ctx.awaits, ctx.agent, ctx.conv, id) {
            Ok(gone) => format!("dropped {} — it was waiting {}", gone.id, gone.describe()),
            Err(e) => format!("{id}: {e}"),
        });
    }
    let left = ctx.awaits.for_conversation(ctx.agent, ctx.conv).len();
    Ok(format!(
        "{}\n{left} await(s) still pending in this conversation.",
        lines.join("\n")
    ))
}

/// Change a wake in place. Omit-to-keep throughout: a field the model did not name is a field it
/// said nothing about, which is what makes "leave everything, just move the deadline" expressible.
fn change_await(id: &str, input: &Value, ctx: &Ctx<'_>) -> std::result::Result<String, String> {
    let change = awaits::Change {
        note: input
            .get("note")
            .and_then(Value::as_str)
            .map(str::to_string),
        events: input.get("events").map(|_| string_list(input, "events")),
        when: match input.get("when") {
            Some(_) => Some(string_map(input, "when")?),
            None => None,
        },
        after_seconds: arg_u64(input, "after_seconds"),
        every_seconds: arg_u64(input, "every_seconds"),
        check: input
            .get("check")
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_in_seconds: arg_u64(input, "expires_in_seconds"),
    };
    let changed = awaits::update(&ctx.awaits, ctx.agent, ctx.conv, id, &change)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "await {} now wakes you {}. Finish this turn.",
        changed.id,
        changed.describe()
    ))
}

/// Write down a decision for a person to make, and tell the turn to end.
///
/// The refusal an unattended agent gets is deliberately an `Err` — the model reads a failed tool
/// result and corrects, and what it must correct here is *stopping at all*. Returned as an `Ok`
/// it would read as "asked, now end your turn", which is exactly the silent halt the flag exists
/// to prevent.
fn ask(input: &Value, ctx: &Ctx<'_>) -> std::result::Result<String, String> {
    if ctx.unattended {
        return Err(
            "this agent runs unattended — nobody is going to see a question, so asking one would \
             stop the work silently. Decide it yourself, carry on, and say plainly in your answer \
             what you assumed and what would change if the assumption is wrong."
                .to_string(),
        );
    }

    let questions = input
        .get("questions")
        .and_then(Value::as_array)
        .ok_or("this tool needs a `questions` array — one entry per thing you need decided")?
        .iter()
        .map(parse_question)
        .collect::<std::result::Result<Vec<_>, String>>()?;

    let req = AskRequest {
        note: input
            .get("note")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        questions,
        after_seconds: arg_u64(input, "after_seconds"),
        defaults: string_list(input, "defaults"),
    };
    let asked = ctx
        .sessions
        .ask(ctx.agent, ctx.conv, &req)
        .map_err(|e| e.to_string())?;

    // Announced on the bus rather than delivered anywhere: this crate has no idea who is watching,
    // and a trigger subscribed to `adi.agents.question.**` is how the question reaches a phone.
    // Best-effort — a question nobody was told about is still a question that got written down.
    let _ = adi_events::Events::with_config(ctx.awaits.config().clone()).emit(
        crate::events::QUESTION_ASKED,
        serde_json::to_string(&crate::events::AgentQuestionAsked {
            agent: ctx.agent.to_string(),
            conv: ctx.conv.to_string(),
            ask: asked.id.clone(),
            question: asked.headline(),
        })
        .unwrap_or_default(),
    );

    let deadline = match asked.deadline {
        Some(_) => " If nobody answers in time, your defaults are taken and you are told so.",
        None => "",
    };
    Ok(format!(
        "asked {} — {} question(s) put to the person you are working for.{deadline} End your turn \
         now; their answer arrives as a new message with this whole conversation still in front of \
         you.",
        asked.id,
        asked.questions.len()
    ))
}

/// One entry of the `questions` array, as the model spelled it.
fn parse_question(value: &Value) -> std::result::Result<crate::store::Question, String> {
    let text = value
        .get("question")
        .and_then(Value::as_str)
        .ok_or("every entry of `questions` needs a `question` string")?;
    let options = value
        .get("options")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|option| {
                    // A bare string where an object was asked for is the commonest way a model
                    // spells a list of choices, and reading it is cheaper than refusing it.
                    let (label, description) = match option {
                        Value::String(label) => (label.as_str(), ""),
                        other => (
                            other.get("label").and_then(Value::as_str)?,
                            other
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        ),
                    };
                    Some(Choice {
                        label: label.trim().to_string(),
                        description: description.trim().to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(crate::store::Question {
        header: value
            .get("header")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        question: text.trim().to_string(),
        options,
        multi_select: value
            .get("multi_select")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

// ---- shared helpers ----------------------------------------------------------------

/// A relative path means "in the directory this run is about"; an absolute one means itself.
fn resolve(path: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// A string-array argument, trimmed, with the empties dropped. Absent reads as empty — a list
/// nobody gave and a list given empty mean the same thing everywhere it is used here.
fn string_list(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A string-to-string map argument, rejecting a value that is not one — a filter quietly dropped
/// for being the wrong shape would leave the model believing its wake was narrowed when it was not.
fn string_map(
    input: &Value,
    key: &str,
) -> std::result::Result<std::collections::BTreeMap<String, String>, String> {
    let Some(value) = input.get(key) else {
        return Ok(std::collections::BTreeMap::new());
    };
    let object = value
        .as_object()
        .ok_or_else(|| format!("`{key}` is an object of field names to the values they must have"))?;
    object
        .iter()
        .map(|(field, want)| match want {
            Value::String(text) => Ok((field.clone(), text.clone())),
            Value::Null | Value::Array(_) | Value::Object(_) => Err(format!(
                "`{key}.{field}` must be the value to match, as a string"
            )),
            other => Ok((field.clone(), other.to_string())),
        })
        .collect()
}

/// A non-empty trimmed string argument, or `None` — how an optional id is read, so `""` and a
/// missing key mean the same thing rather than one of them naming an await called nothing.
fn arg_id<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

fn arg_str<'a>(input: &'a Value, key: &str) -> std::result::Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("this tool needs a `{key}` string argument"))
}

/// A number argument, however the model spelled it — `2` and `2.0` mean the same line here.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the float is range-checked against u64 before it is cast"
)]
fn arg_u64(input: &Value, key: &str) -> Option<u64> {
    let v = input.get(key)?;
    v.as_u64().or_else(|| {
        v.as_f64()
            // 2^63 rather than u64::MAX: the comparison is in floats, and u64::MAX has no exact
            // float form. No line number or timeout is anywhere near either.
            .filter(|f| f.is_finite() && *f >= 0.0 && *f < 9.223_372_036_854_776e18)
            .map(|f| f.round() as u64)
    })
}

/// Cut an over-long result down, saying so where the model will see it — silence here reads as
/// "that was the whole file", which is how an agent ends up editing text that isn't there.
pub(crate) fn truncate(text: &str) -> String {
    if text.len() <= MAX_OUTPUT {
        return text.to_string();
    }
    let mut cut = MAX_OUTPUT;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!(
        "{}\n\n[… {} more bytes. Narrow the request — an offset/limit, a tighter pattern, or a \
         more specific path.]",
        &text[..cut],
        text.len() - cut
    )
}

/// Depth-first walk, skipping the directories nobody means ([`SKIP_DIRS`]) and anything hidden.
fn walk(root: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        if path.is_dir() {
            walk(&path, visit);
        } else {
            visit(&path);
        }
    }
}

/// Match `path` against a glob: `?` is one character, `*` any run within a segment, `**` any number
/// of segments. A pattern with no `/` matches the file name alone, so `*.rs` finds Rust files at
/// any depth — which is what everyone means by it.
fn glob_match(pattern: &str, path: &str) -> bool {
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        return glob_segments(pattern, name);
    }
    glob_segments(pattern, path)
}

fn glob_segments(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_here(&p, &t)
}

/// Backtracking matcher over the two character slices. Small inputs (a pattern and one path), so
/// the simple recursive form is the honest one — no table, nothing to get subtly wrong.
fn glob_here(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            // `**` crosses `/`; a single `*` stops at one.
            let (rest, crosses) = if p.get(1) == Some(&'*') {
                // Swallow the `/` that usually follows `**`, so `**/x` also matches a bare `x`.
                let after = if p.get(2) == Some(&'/') { &p[3..] } else { &p[2..] };
                (after, true)
            } else {
                (&p[1..], false)
            };
            if glob_here(rest, t) {
                return true;
            }
            for i in 0..t.len() {
                if !crosses && t[i] == '/' {
                    break;
                }
                if glob_here(rest, &t[i + 1..]) {
                    return true;
                }
            }
            false
        }
        '?' if !t.is_empty() && t[0] != '/' => glob_here(&p[1..], &t[1..]),
        c if !t.is_empty() && t[0] == c => glob_here(&p[1..], &t[1..]),
        _ => false,
    }
}

/// Run `cmd` to completion, killing it if it outstays `timeout_ms`. `std` has no timed wait, so
/// this polls — cheaply, and only while a command is actually running.
///
/// Shared with [`crate::awaits`], whose checks are shell commands under a deadline for the same
/// reason `Bash` is: an unbounded one would hold whoever is waiting on it for ever.
///
/// # Both streams are drained while the command runs, not after
///
/// This is the whole reason the function is more than a poll loop. A pipe holds about 64 KiB; a
/// child that fills it blocks in `write` and never exits, so a reader that waits for the exit before
/// reading is waiting for something that cannot happen. Measured before this drained: `yes | head
/// -40000` — a tenth of a second of work, a megabyte of output — never finished, and came back as
/// "still running after 8000ms and was killed". Every chatty build and test suite is that command.
///
/// The deadlock also reached further than the caller. An await's check runs on the app's *single*
/// await worker, one at a time, so a check that printed a megabyte stalled every other pending wake
/// on the machine until its 20-second deadline expired.
///
/// So each stream gets a thread that reads until the pipe closes. They keep only the first
/// [`MAX_CAPTURE`] bytes and go on draining past that — the cap bounds this process's memory, and
/// continuing to read is what keeps the child unblocked. Both threads end when the child does,
/// including when it is killed, so nothing is left behind.
pub(crate) fn wait_with_timeout(
    mut cmd: Command,
    timeout_ms: u64,
) -> std::result::Result<std::process::Output, String> {
    use std::process::Stdio;

    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start the shell: {e}"))?;

    // Taken before the wait, so the pipes are being emptied from the first byte the child writes.
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "the command was still running after {timeout_ms}ms and was killed — set \
                     `background` to let it run past this call, or raise timeout_ms"
                ));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("couldn't wait for the shell: {e}"));
            }
        }
    };

    // Collected with a grace period rather than joined, and that distinction is the difference
    // between a deadline that holds and one that can be walked straight past. A pipe stays open
    // while *any* writer holds it, and a command is free to leave one behind: `(sleep 45 &)` exits
    // in milliseconds having handed its stdout to an orphan. Joining there waits for the orphan —
    // measured at 45 seconds on a call whose timeout was 3. So once the child is gone the readers
    // get a moment to finish, and whatever they have is taken either way.
    let (stdout, stderr) = (collect(stdout), collect(stderr));

    outcome.map(|status| std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// How much of one stream is kept. Well above [`MAX_OUTPUT`], which is what actually reaches the
/// model — the gap is there so the truncation the model is told about happens in one place, on text
/// this has already read in full.
const MAX_CAPTURE: usize = 1 << 20;

/// How long a reader is given to finish once the child is gone. The pipe closes with the child in
/// every ordinary case, so this is the time a thread needs to notice — not a wait anybody plans on.
const DRAIN_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// A stream being read on a thread of its own, and the buffer it is filling.
struct Drain {
    kept: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    handle: std::thread::JoinHandle<()>,
}

/// Read `stream` to its end on a thread of its own, keeping the first [`MAX_CAPTURE`] bytes.
///
/// Reading *past* the cap is the point rather than an oversight: stopping early would leave the pipe
/// to fill and the child blocked in `write`, which is the deadlock this exists to prevent.
///
/// What is read lands in a shared buffer as it arrives rather than being returned at the end, so a
/// reader still blocked on a pipe an orphan is holding can be abandoned without losing the output it
/// already has.
fn drain(mut stream: impl std::io::Read + Send + 'static) -> Drain {
    let kept = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writing = std::sync::Arc::clone(&kept);
    let handle = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // Held only long enough to append: the reader must never wait on whoever is
                    // reading the buffer, and the buffer's owner must never wait on the pipe.
                    if let Ok(mut kept) = writing.lock()
                        && kept.len() < MAX_CAPTURE
                    {
                        let room = MAX_CAPTURE - kept.len();
                        kept.extend_from_slice(&buf[..n.min(room)]);
                    }
                }
            }
        }
    });
    Drain { kept, handle }
}

/// What a reader has, once it has had [`DRAIN_GRACE`] to finish.
///
/// A thread still blocked after that is left to end on its own — it holds a bounded buffer and exits
/// the moment the last writer closes the pipe. Abandoning it is what keeps a stray orphan from
/// deciding how long this call takes.
fn collect(drain: Option<Drain>) -> Vec<u8> {
    let Some(drain) = drain else {
        return Vec::new();
    };
    let deadline = std::time::Instant::now() + DRAIN_GRACE;
    while !drain.handle.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    drain
        .kept
        .lock()
        .map(|kept| kept.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("adi-tools-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// A tool context whose await and session stores are both scratch, so registering a wake or a
    /// question in a test never reaches the real ones.
    fn ctx_in<'a>(cwd: &'a Path, tag: &str) -> Ctx<'a> {
        let root =
            std::env::temp_dir().join(format!("adi-tools-awaits-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Ctx {
            cwd,
            shell: Shell::new(cwd, "conv-1"),
            agent: "watcher",
            conv: "conv-1",
            awaits: Awaits::with_config(adi_config::Config::with_root(root.join("await-store"))),
            sessions: SessionStore::new(root.join("sessions")),
            agent_dir: cwd,
            unattended: false,
        }
    }

    /// A scratch store with one conversation already open in it.
    ///
    /// An ask is a row that cascades off its session's, so there is nowhere to file a question in a
    /// conversation nobody opened. The id is minted by the store rather than chosen, which is why
    /// this owns the pieces and hands out a borrowed [`Ctx`] instead of being one.
    struct Conversation {
        root: std::path::PathBuf,
        sessions: SessionStore,
        awaits: Awaits,
        conv: String,
    }

    impl Conversation {
        fn open(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("adi-tools-ask-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let sessions = SessionStore::new(root.join("sessions"));
            let conv = sessions
                .create("watcher", crate::Backend::HarnessAdi, &root, "go")
                .expect("open a conversation")
                .id;
            Self {
                awaits: Awaits::with_config(adi_config::Config::with_root(root.join("awaits"))),
                sessions,
                conv,
                root,
            }
        }

        fn ctx(&self) -> Ctx<'_> {
            Ctx {
                cwd: &self.root,
                shell: Shell::new(&self.root, &self.conv),
                agent: "watcher",
                conv: &self.conv,
                awaits: self.awaits.clone(),
                sessions: self.sessions.clone(),
                agent_dir: &self.root,
                unattended: false,
            }
        }
    }

    impl Drop for Conversation {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn question(text: &str) -> Value {
        json!({
            "header": "Choice",
            "question": text,
            "options": [{ "label": "yes", "description": "do it" }, "no"],
        })
    }

    #[test]
    fn a_bare_pattern_matches_the_file_name_at_any_depth() {
        assert!(glob_match("*.rs", "src/deep/lib.rs"));
        assert!(glob_match("**/*.rs", "src/deep/lib.rs"));
        assert!(glob_match("src/**/*.rs", "src/deep/lib.rs"));
        assert!(glob_match("**/lib.rs", "lib.rs"));
        assert!(!glob_match("src/*.rs", "src/deep/lib.rs"));
        assert!(!glob_match("*.toml", "src/lib.rs"));
    }

    #[test]
    fn edit_refuses_an_ambiguous_match_and_says_how_many() {
        let dir = scratch("edit");
        let file = dir.join("f.txt");
        std::fs::write(&file, "a\na\n").expect("write");
        let input = json!({"path": "f.txt", "old_string": "a", "new_string": "b"});
        let err = edit(&input, &dir).expect_err("ambiguous edit must fail");
        assert!(err.contains("appears 2 times"), "{err}");

        let all = json!({"path": "f.txt", "old_string": "a", "new_string": "b", "replace_all": true});
        edit(&all, &dir).expect("replace_all");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "b\nb\n");
    }

    #[test]
    fn a_missing_old_string_tells_the_model_what_to_do_about_it() {
        let dir = scratch("edit-missing");
        std::fs::write(dir.join("f.txt"), "hello\n").expect("write");
        let input = json!({"path": "f.txt", "old_string": "nope", "new_string": "x"});
        let err = edit(&input, &dir).expect_err("missing text must fail");
        assert!(err.contains("read the file"), "{err}");
    }

    #[test]
    fn read_resolves_a_relative_path_against_the_runs_own_directory() {
        let dir = scratch("read");
        std::fs::write(dir.join("f.txt"), "one\ntwo\nthree\n").expect("write");
        let all = read(&json!({"path": "f.txt"}), &dir).expect("read");
        assert_eq!(all, "one\ntwo\nthree");
        let windowed = read(&json!({"path": "f.txt", "offset": 2, "limit": 1}), &dir).expect("read");
        assert_eq!(windowed, "two");
    }

    /// A background call is two things in one act: the job starts, and the wake that will report it
    /// is registered. Registering separately was the alternative, and it would mean a model that
    /// forgot the second call had started work nothing would ever tell it about.
    #[cfg(unix)]
    #[test]
    fn a_background_command_starts_a_job_and_registers_the_wake_in_one_call() {
        let dir = scratch("bash-bg");
        let ctx = ctx_in(&dir, "bash-bg");

        let said = bash(
            &json!({ "command": "echo from-the-job", "background": true }),
            &ctx,
        )
        .expect("a job starts");
        assert!(said.contains("in the background"), "{said}");
        assert!(said.contains("job-"), "the model is told the id: {said}");
        assert!(said.contains(".log"), "and where the output goes: {said}");
        assert!(said.contains("woken"), "{said}");

        let pending = ctx.awaits.for_conversation("watcher", "conv-1");
        assert_eq!(pending.len(), 1, "exactly one wake: {pending:?}");
        let wake = &pending[0];
        assert!(
            wake.check.as_deref().is_some_and(|c| c.contains("job-")),
            "the wake watches this job: {wake:?}"
        );
        assert!(
            wake.note.contains("echo from-the-job"),
            "and reminds the run what it started: {wake:?}"
        );
        assert_eq!(wake.every, Some(jobs::LOOK_EVERY_SECONDS));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `timeout_ms` is the foreground contract and says nothing about a job — a background call
    /// returns at once whatever it was given, rather than being killed by a deadline it outlives.
    #[cfg(unix)]
    #[test]
    fn a_background_command_is_not_bound_by_the_foreground_timeout() {
        let dir = scratch("bash-bg-timeout");
        let ctx = ctx_in(&dir, "bash-bg-timeout");
        let said = bash(
            &json!({ "command": "sleep 30", "background": true, "timeout_ms": 200 }),
            &ctx,
        )
        .expect("a job starts");
        assert!(said.contains("in the background"), "{said}");
        assert!(!said.contains("still running"), "not a timeout: {said}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pipe holds about 64 KiB. A reader that waits for the child to exit before emptying it is
    /// waiting for a child that is blocked in `write` — so a chatty command used to run its whole
    /// timeout and come back killed. The timeout here is generous and the assertion is on the clock:
    /// this passes in well under a second and only fails by taking all ten.
    #[cfg(unix)]
    #[test]
    fn a_command_that_outtalks_the_pipe_buffer_still_finishes() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("yes abcdefghijklmnop | head -60000");

        let began = std::time::Instant::now();
        let out = wait_with_timeout(cmd, 10_000).expect("a loud command is not a hung one");
        assert!(out.status.success());
        assert!(
            out.stdout.len() > 256 * 1024,
            "the whole stream is read, not one buffer's worth: {}",
            out.stdout.len()
        );
        assert!(
            began.elapsed() < std::time::Duration::from_secs(5),
            "it finished on its own rather than on the deadline: {:?}",
            began.elapsed()
        );
    }

    /// The deadline has to survive a command that leaves a writer behind. `(sleep &)` exits at once
    /// but hands its stdout to an orphan, and a pipe stays open while *any* writer holds it — so
    /// reading to the end here means reading until the orphan is done, whatever the timeout said.
    #[cfg(unix)]
    #[test]
    fn an_orphan_holding_the_pipe_does_not_decide_how_long_the_call_takes() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("(sleep 30 &) ; echo parent-done");

        let began = std::time::Instant::now();
        let out = wait_with_timeout(cmd, 60_000).expect("the command itself succeeded");
        assert!(
            began.elapsed() < std::time::Duration::from_secs(5),
            "the call returns with its child, not with the orphan: {:?}",
            began.elapsed()
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("parent-done"),
            "{:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    #[test]
    fn a_nonzero_exit_comes_back_as_a_result_not_an_error() {
        let dir = scratch("bash");
        let out = bash(&json!({"command": "echo out; exit 3"}), &ctx_in(&dir, "bash"))
            .expect("nonzero is a result");
        assert!(out.contains("exit 3"), "{out}");
        assert!(out.contains("out"), "{out}");
    }

    #[test]
    fn a_command_that_never_finishes_is_killed_and_says_so() {
        let dir = scratch("bash-timeout");
        let err = bash(&json!({"command": "sleep 5", "timeout_ms": 200}), &ctx_in(&dir, "bash"))
            .expect_err("a hung command must fail");
        assert!(err.contains("still running"), "{err}");
    }

    /// What the model is told about a `cd`: the shell moved, the file tools did not. Reported only
    /// when it happens, so an ordinary command's output stays the whole of the answer.
    #[test]
    #[cfg(unix)]
    fn a_command_that_moves_the_shell_says_where_it_left_it() {
        let dir = scratch("bash-cd");
        std::fs::create_dir_all(dir.join("workspaces")).expect("mkdir");
        let inner = std::fs::canonicalize(dir.join("workspaces")).expect("canonicalize");
        let inner = inner.to_str().expect("utf8");
        let stayed = bash(&json!({"command": "echo here"}), &ctx_in(&dir, "bash")).expect("run");
        assert!(!stayed.contains("the shell is now in"), "{stayed}");

        let moved = bash(&json!({"command": "cd workspaces"}), &ctx_in(&dir, "bash")).expect("run");
        assert!(moved.contains(inner), "the move is reported: {moved}");
        let after = bash(&json!({"command": "pwd -P"}), &ctx_in(&dir, "bash")).expect("run");
        assert!(after.contains(inner), "{after}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grep_reports_path_and_line_and_honours_its_glob() {
        let dir = scratch("grep");
        std::fs::write(dir.join("a.rs"), "fn wanted() {}\n").expect("write");
        std::fs::write(dir.join("b.txt"), "wanted\n").expect("write");
        let hits = grep(&json!({"pattern": "wanted", "glob": "*.rs"}), &dir).expect("grep");
        assert!(hits.contains("a.rs:1:"), "{hits}");
        assert!(!hits.contains("b.txt"), "{hits}");
    }

    #[test]
    fn an_unknown_tool_names_the_ones_that_exist() {
        let ctx = ctx_in(Path::new("."), "unknown-tool");
        let err = execute("Frobnicate", &json!({}), &ctx).expect_err("unknown tool");
        assert!(err.contains("Read"), "{err}");
        assert!(err.contains("Await"), "the wake tool is advertised too: {err}");
        assert!(err.contains("Ask"), "and so is the question tool: {err}");
    }

    /// The wake tool's arguments reach the store, and a request that can never fire comes back as a
    /// failed tool result the model can read and correct — not as a failure of the turn.
    #[test]
    fn await_registers_what_the_model_asked_for_and_explains_a_bad_request() {
        let dir = scratch("await");
        let ctx = ctx_in(&dir, "await");
        let ok = await_wake(
            &json!({ "note": "check the deploy", "events": ["adi.tasks.*"], "check": "true" }),
            &ctx,
        )
        .expect("register");
        assert!(ok.contains("adi.tasks.*"), "{ok}");
        assert!(ok.contains("if the check passes"), "{ok}");

        let err = await_wake(&json!({ "note": "waiting" }), &ctx).expect_err("must be refused");
        assert!(err.contains("something to wake on"), "{err}");
        let err = await_wake(&json!({ "events": ["adi.tasks.*"] }), &ctx).expect_err("no note");
        assert!(err.contains("`note`"), "{err}");

        let pending = ctx.awaits.for_conversation("watcher", "conv-1");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].note, "check the deploy");
        assert_eq!(pending[0].cwd, dir.display().to_string());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The two verbs that act on a wake the run already holds — including one an action registered
    /// for it. A wake it cannot drop is a slot it cannot get back; a wake it cannot change is one it
    /// has to drop and re-register, leaving a gap the thing it is waiting on can finish inside.
    #[test]
    fn await_drops_and_changes_the_wakes_this_conversation_holds() {
        let dir = scratch("await-verbs");
        let ctx = ctx_in(&dir, "await-verbs");
        let registered = await_wake(
            &json!({
                "note": "the run you started ended",
                "events": ["adi.agents.run.finished"],
                "when": { "run_id": "r-42" },
            }),
            &ctx,
        )
        .expect("register");
        assert!(registered.contains("run_id=r-42"), "the filter is in what it reads: {registered}");

        let id = ctx.awaits.for_conversation("watcher", "conv-1")[0].id.clone();

        let changed = await_wake(
            &json!({ "update": id, "note": "why I am waiting", "expires_in_seconds": 600 }),
            &ctx,
        )
        .expect("update");
        assert!(changed.contains(&id), "the id survives a change: {changed}");
        let pending = ctx.awaits.for_conversation("watcher", "conv-1");
        assert_eq!(pending.len(), 1, "changed in place, not registered again");
        assert_eq!(pending[0].note, "why I am waiting");
        assert_eq!(pending[0].events, vec!["adi.agents.run.finished".to_string()]);
        assert!(pending[0].expiry_wakes, "a deadline it named is one it hears about");

        let dropped = await_wake(&json!({ "ignore": [&id] }), &ctx).expect("ignore");
        assert!(dropped.contains("dropped"), "{dropped}");
        assert!(dropped.contains("0 await(s) still pending"), "{dropped}");
        assert!(ctx.awaits.for_conversation("watcher", "conv-1").is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An id the conversation does not hold is a failed tool result naming the ones it does — a
    /// model reaching for the wrong id is usually holding one that has already fired.
    #[test]
    fn await_answers_an_unknown_id_with_what_is_actually_pending() {
        let dir = scratch("await-unknown");
        let ctx = ctx_in(&dir, "await-unknown");
        await_wake(&json!({ "note": "waiting", "events": ["adi.tasks.*"] }), &ctx).expect("register");
        let id = ctx.awaits.for_conversation("watcher", "conv-1")[0].id.clone();

        let err = await_wake(&json!({ "update": "w-nope", "note": "x" }), &ctx).expect_err("unknown");
        assert!(err.contains(&id), "the real one is named: {err}");
        // `ignore` reports per id rather than failing the call, so a run dropping several is not
        // stopped by one that has already fired.
        let mixed = await_wake(&json!({ "ignore": ["w-nope"] }), &ctx).expect("reported, not failed");
        assert!(mixed.contains(&id), "{mixed}");
        assert!(mixed.contains("1 await(s) still pending"), "{mixed}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What the model wrote reaches the store intact, and what it gets back tells it the one thing
    /// it has to do next: stop.
    #[test]
    fn ask_records_the_question_and_tells_the_turn_to_end() {
        let chat = Conversation::open("records");
        let ctx = chat.ctx();

        let reply = ask(
            &json!({
                "note": "two ways to do the migration",
                "questions": [question("in place, or a new table?")],
            }),
            &ctx,
        )
        .expect("asked");
        assert!(reply.contains("End your turn"), "{reply}");

        let pending = ctx
            .sessions
            .pending_question("watcher", &chat.conv)
            .expect("the conversation is blocked on it");
        assert_eq!(pending.note, "two ways to do the migration");
        assert_eq!(pending.questions.len(), 1);
        assert_eq!(pending.questions[0].options.len(), 2);
        assert_eq!(pending.questions[0].options[1].label, "no");
        assert!(pending.deadline.is_none(), "no deadline was named");
    }

    /// A deadline is a statement about what happens when nobody answers — so it is only a deadline
    /// if the run says what it will assume. Both halves reach the store together or not at all.
    #[test]
    fn ask_takes_a_deadline_only_with_the_assumption_to_go_with_it() {
        let chat = Conversation::open("deadline");
        let ctx = chat.ctx();

        let err = ask(
            &json!({ "questions": [question("deploy?")], "after_seconds": 600 }),
            &ctx,
        )
        .expect_err("refused");
        assert!(err.contains("defaults"), "{err}");

        let reply = ask(
            &json!({
                "questions": [question("deploy?")],
                "after_seconds": 600,
                "defaults": ["no — leave it for a human"],
            }),
            &ctx,
        )
        .expect("asked");
        assert!(reply.contains("defaults are taken"), "{reply}");
        assert!(
            ctx.sessions
                .pending_question("watcher", &chat.conv)
                .expect("pending")
                .deadline
                .is_some()
        );
    }

    /// Nobody is watching an unattended run, so a question there is not a question — it is the work
    /// stopping without failing. The refusal is an `Err` on purpose: the model reads a failed tool
    /// result and corrects, and what it must correct is stopping at all.
    #[test]
    fn an_unattended_agent_is_told_to_decide_for_itself() {
        let chat = Conversation::open("unattended");
        let ctx = Ctx {
            unattended: true,
            ..chat.ctx()
        };

        let err = ask(&json!({ "questions": [question("deploy?")] }), &ctx).expect_err("refused");
        assert!(err.contains("unattended"), "{err}");
        assert!(
            err.contains("what you assumed"),
            "and it is told what to do instead: {err}"
        );
        assert!(
            ctx.sessions
                .pending_question("watcher", &chat.conv)
                .is_none(),
            "nothing was written down"
        );
    }

    /// The malformed cases, each answered in the voice of the tool rather than as a failed turn.
    #[test]
    fn a_malformed_ask_explains_itself_to_the_model() {
        let chat = Conversation::open("malformed");
        let ctx = chat.ctx();

        let err = ask(&json!({ "note": "hello" }), &ctx).expect_err("no questions");
        assert!(err.contains("`questions`"), "{err}");

        let err = ask(&json!({ "questions": [json!({ "header": "x" })] }), &ctx)
            .expect_err("a question with no text");
        assert!(err.contains("`question`"), "{err}");

        let err = ask(
            &json!({ "questions": (0..=MAX_QUESTIONS).map(|_| question("q?")).collect::<Vec<_>>() }),
            &ctx,
        )
        .expect_err("too many");
        assert!(err.contains(&MAX_QUESTIONS.to_string()), "{err}");
    }
}
