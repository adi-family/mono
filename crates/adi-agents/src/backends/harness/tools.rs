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

use adi_events::Event as _;
use serde_json::{Value, json};

use crate::awaits::{self, Awaits, Request};
#[cfg(unix)]
use crate::backends::interpreter::Language;
use crate::backends::jobs;
use crate::backends::mcp_hub::Hub;
use crate::backends::shell::Shell;
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
    /// Which of the code tools this agent has switched on — see [`CodeTools`].
    pub code: CodeTools,
    /// The agent's own MCP servers, for the one kind of agent whose engine is not their client:
    /// a code-only one, whose scripts call them (see [`crate::backends::mcp_hub`]). Empty for
    /// every other agent — an engine that was handed the servers talks to them itself.
    pub mcp: Hub,
}

/// Which of the code tools — `PyCode`, `TsCode` — an agent has.
///
/// Neither, unless its definition says so ([`AgentManifest::pycode`](crate::AgentManifest::pycode),
/// [`tscode`](crate::AgentManifest::tscode)). They are switches on the agent rather than rows
/// everybody is served for the reason [`super::super::mcp::scope_tools`] gives for the engine's own
/// tools: a capability nobody asked for is one nobody decided to grant. Each is also a long-lived
/// process per conversation, and two thousand tokens of description in every request — a cost an
/// agent that will never write a line of Python should not be paying.
///
/// A tool that is off is *absent*: not declared to the model, not listed over MCP, and answered
/// as any unknown name is if called anyway.
///
/// # Code-only
///
/// [`only`](Self::only) turns the arrangement inside out: the one code tool that is on is the
/// *only* tool the agent has, and every other row of the table is a function its code calls
/// rather than a tool the model is declared (see [`crate::backends::code_only`]). It is the
/// agent's [`code_only`](crate::AgentManifest::code_only) setting, which also picks the language
/// — so with it set, exactly one of the two switches above is on, whatever the agent's own
/// `pycode` and `tscode` say.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodeTools {
    /// `PyCode`.
    pub python: bool,
    /// `TsCode`.
    pub typescript: bool,
    /// The code tool that is on is the only tool there is.
    pub only: bool,
}

impl CodeTools {
    /// Both on — for a test, and for nothing else.
    #[cfg(test)]
    pub(crate) const BOTH: Self = Self {
        python: true,
        typescript: true,
        only: false,
    };

    /// Code-only, in TypeScript.
    pub const ONLY_TYPESCRIPT: Self = Self {
        python: false,
        typescript: true,
        only: true,
    };

    /// Code-only, in Python.
    pub const ONLY_PYTHON: Self = Self {
        python: true,
        typescript: false,
        only: true,
    };

    /// Whether an agent with these switches has the tool called `name`. A code tool is one it
    /// has when it is switched on; every other tool is one it has unless it is code-only, where
    /// the others are functions and not tools.
    #[must_use]
    pub fn offers(self, name: &str) -> bool {
        match name {
            "PyCode" => self.python,
            "TsCode" => self.typescript,
            _ => !self.only,
        }
    }
}

/// Whether `name` is one of the code tools — the two rows of the table that run code rather
/// than being called from it.
#[cfg(unix)]
fn is_code_tool(name: &str) -> bool {
    matches!(name, "PyCode" | "TsCode")
}

/// The tools code can call as functions: every row of the table but the code tools themselves.
/// What a code-only agent has instead of a tool list.
#[cfg(unix)]
pub(crate) fn inner() -> impl Iterator<Item = &'static ToolSpec> {
    TOOLS.iter().filter(|spec| !is_code_tool(spec.name))
}

/// The tools an agent with these switches has, in the order they are advertised.
pub(crate) fn offered(code: CodeTools) -> impl Iterator<Item = &'static ToolSpec> {
    TOOLS.iter().filter(move |spec| code.offers(spec.name))
}

/// The one sentence of [`BASH`] that is not true from inside code: there, the file tools resolve
/// where the code stands, which is where the shell stands. A macro only because `concat!` takes
/// literals; [`BASH_PATHS`] is the same text for whoever needs to take it back out.
#[cfg(unix)]
macro_rules! bash_paths {
    () => {
        "Only the shell moves: Read, Write, Edit, Glob and Grep go on resolving relative paths \
         against the run's own directory. "
    };
}
/// See [`bash_paths`].
#[cfg(unix)]
pub(crate) const BASH_PATHS: &str = bash_paths!();

/// What `Bash` is, told in the terms that decide how a command gets written: the shell is the
/// conversation's, not the call's. Two texts because that is only true where [`super::shell`] can
/// keep it — a model on Windows told it could name a path once would name it into nothing.
#[cfg(unix)]
const BASH: &str = concat!(
    "Run a shell command and return its output (stdout and stderr together, with \
                    the exit status when it is not zero). It is one shell across the whole \
                    conversation: `cd` moves it until you move it again, and an **exported** \
                    variable is still set on your next call. So name a long path once — `export \
                    FE=/long/path/to/a/checkout` — and use `$FE` from then on, rather than writing \
                    it out on command after command. A bare `FE=…` is not exported and does not \
                    carry. ",
    bash_paths!(),
    "Set \
                    `background` for a command that will outlast the turn: it is started and you \
                    get a job id back straight away, then **end your turn** — you are woken when it \
                    exits, carrying its exit status and the tail of its output. That is how to run \
                    a build or a test suite without sitting inside the call waiting for it. Do not \
                    poll a job you started; being woken is the whole point. Never `background` \
                    `adi-mono agents run` — it already returns in about a second and hands you its \
                    own await for the run it started; backgrounding it only adds a second, \
                    redundant wake for the launcher exiting."
);
#[cfg(windows)]
const BASH: &str = "Run a shell command in the working directory and return its output (stdout \
                    and stderr together, with the exit status when it is not zero). Each call is \
                    its own shell, so a `cd` or a variable you set is gone by the next one — use \
                    full paths.";

/// What `PyCode` is, told in the terms that decide how a cell gets written. The first paragraph is
/// the reason the tool exists — state that outlives the call — and it leads because a model that
/// misses it writes Python the way it writes shell: everything fetched again, everything printed.
/// The rest is what it could not guess: that `sh()` raises, what a cell shows, that the directory
/// is the shell's, that a background cell is a different interpreter, and how a value gets out
/// of this one.
#[cfg(unix)]
const PYCODE: &str = "Run Python in this conversation's own interpreter and return what it \
                      printed. The interpreter stays up between calls: a variable you set, a \
                      module you imported, a function you defined is still there on your next \
                      call. So read or fetch a thing **once**, keep it in a variable, and work on \
                      it from there — rather than printing all of it, or fetching it again. \
                      The value of the cell's last expression is shown, as in a notebook, so end \
                      with a bare `len(rows)` rather than a `print`. An exception ends the call \
                      with its traceback; what ran before it has happened, and the interpreter and \
                      your variables survive it.\n\n`sh(cmd)` runs a command and returns its \
                      stdout as a string, trailing newline dropped, with `.stderr`, `.code`, `.ok` \
                      and `.lines` beside it — `head = sh('git rev-parse HEAD')`. A string runs \
                      through the shell; a list (`sh(['git', 'commit', '-m', msg])`) runs as \
                      written, with no quoting to get wrong. A command that exits non-zero raises \
                      `ShError` carrying its output, so a cell of several steps stops at the one \
                      that broke — pass `check=False` where failing is an answer (`grep`, `diff`, \
                      `test`). A bare `sh(...)` line shows the command's output wherever it stands \
                      in the cell; `echo=True` shows it as it is produced. `os`, `sys`, `re`, \
                      `json`, `subprocess`, `shutil`, `time` and `Path` are already imported.\n\n\
                      It stands where your shell stands, and moves it: `os.chdir()` moves Bash \
                      too, what you put in `os.environ` is exported to it, and wherever Bash `cd`'d \
                      or whatever it exported is where this starts. Only variables and imports are \
                      the interpreter's own. It is kept for an hour without a call; if it is ever \
                      gone, the result says so, and what it held has to be built again. \
                      `share('name', value)` keeps a JSON-able value outside the interpreter, \
                      where `shared('name')` finds it again: after a restart, from a background \
                      cell, and from TsCode if you have it.\n\nSet \
                      `background` for code that will outlast the turn: it runs as a script of its \
                      own — same directory, same environment, same `sh` — and you **end your \
                      turn** and are woken when it exits, with its status and the tail of its \
                      output. A background script does not see your variables and nothing it \
                      defines comes back, so have it `share` its result or write it to a file.";

/// What `TsCode` is. The same tool as [`PYCODE`] in another language, and told in the same order
/// for the same reason — but not the same text, because the differences are exactly what a model
/// would get wrong by assuming: everything here is `await`ed, only the last expression is shown,
/// a missing semicolon changes what a cell means, and a cell that will not stop cannot be made to.
#[cfg(unix)]
const TSCODE: &str = "Run TypeScript in this conversation's own interpreter (Bun) and return what \
                      it printed. The interpreter stays up between calls: what a cell declares at \
                      its top level — a `const`, a function, a class, an import — is still there \
                      on your next call, and may be declared again. So read or fetch a thing \
                      **once**, keep it in a variable, and work on it from there — rather than \
                      printing all of it, or fetching it again. Top-level `await` works. The value \
                      of the cell's last expression is shown, as in a notebook, so end with a bare \
                      `rows.length;` rather than a `console.log`. Write semicolons: a line that \
                      begins with `(` or `[` otherwise continues the line above it. An exception \
                      ends the call with the error and the line it came from; what ran before it \
                      has happened, and the interpreter and your variables survive it.\n\n`await \
                      sh(cmd)` runs a command and resolves to its result: `.text` is stdout with \
                      the trailing newline dropped (and what the result becomes inside a template \
                      string), with `.stdout`, `.stderr`, `.code`, `.ok`, `.lines` and `.json()` \
                      beside it — `const head = (await sh('git rev-parse HEAD')).text;`. A string \
                      runs through the shell; an array (`sh(['git', 'commit', '-m', msg])`) runs \
                      as written, with no quoting to get wrong. A command that exits non-zero \
                      rejects with `ShError` carrying its output, so a cell of several steps stops \
                      at the one that broke — pass `{ check: false }` where failing is an answer \
                      (`grep`, `diff`, `test`). A bare `await sh(...);` line shows the command's \
                      output wherever it stands in the cell; `{ echo: true }` shows it as it is \
                      produced. `fs`, `path` and `os` are already imported, every `Bun.*` API and \
                      Bun's `$` are there, and `import` — static or dynamic — resolves from the \
                      directory you are in and re-reads a file of yours that has changed.\n\nIt \
                      stands where your shell stands, and moves it: `process.chdir()` moves Bash \
                      too, what you put in `process.env` is exported to it, and wherever Bash \
                      `cd`'d or whatever it exported is where this starts. Only what a cell \
                      declared is the interpreter's own. It is kept for an hour without a call; if \
                      it is ever gone, the result says so, and what it held has to be built \
                      again. `share('name', value)` keeps a JSON-able value outside the \
                      interpreter, where `shared('name')` finds it again: after a restart, from a \
                      background cell, and from PyCode if you have it.\n\nRunning JavaScript cannot be interrupted. Past `timeout_ms` the \
                      command the code is waiting on is stopped, which ends most slow cells with \
                      everything else intact — but code still running after that (a long loop, a \
                      wait on something that is not a command) costs the interpreter and every \
                      variable in it. So set `background` for code that will outlast the turn: it \
                      runs as a script of its own — same directory, same environment, same `sh` — \
                      and you **end your turn** and are woken when it exits, with its status and \
                      the tail of its output. A background script does not see your variables and \
                      nothing it defines comes back, so have it `share` its result or write it to \
                      a file.";

/// Everything a turn may call. Order is the order they're advertised, which is the order a model
/// tends to consider them in: look before you write, and shell out only when nothing else fits.
///
/// Not every agent is advertised all of it: see [`offered`] for the two that are switched on per
/// agent.
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
                    "background": { "type": "boolean", "description": "Start the command and return immediately with a job id instead of waiting for it. Use it for anything that outlasts a couple of minutes — a build, a test suite, a deploy. Don't background `adi-mono agents run` — it already returns in about a second and hands you its own await for the run; backgrounding it only adds a second, redundant wake for the launcher exiting. You are woken when it exits, with its status and the tail of its log." },
                },
                "required": ["command"],
            })
        },
    },
    // Declared only where they can run — see `backends::interpreter` for why that is Unix. A tool
    // that is advertised and then refuses every call is worse than one that is not there.
    #[cfg(unix)]
    ToolSpec {
        name: "PyCode",
        description: PYCODE,
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "The Python to run. A cell, not a line: several statements, definitions and loops are all fine." },
                    "timeout_ms": { "type": "integer", "description": "Interrupt the code after this long. Defaults to 120000. An interrupt leaves the interpreter and your variables intact. Ignored when `background` is set — a job has no deadline." },
                    "background": { "type": "boolean", "description": "Run the code as a script of its own and return immediately with a job id instead of waiting for it. Use it for anything that outlasts a couple of minutes. It starts where you stand but in a new interpreter, without your variables. You are woken when it exits, with its status and the tail of its log." },
                    "restart": { "type": "boolean", "description": "Start from a new interpreter first, dropping every variable and import. For when what it holds is in your way — it is never needed just because the last call raised." },
                },
                "required": ["code"],
            })
        },
    },
    #[cfg(unix)]
    ToolSpec {
        name: "TsCode",
        description: TSCODE,
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "The TypeScript to run. A cell, not a line: several statements, declarations and loops are all fine, and so is plain JavaScript." },
                    "timeout_ms": { "type": "integer", "description": "Stop the command the code is waiting on after this long. Defaults to 120000. Code that is still running after that cannot be interrupted and costs the interpreter. Ignored when `background` is set — a job has no deadline." },
                    "background": { "type": "boolean", "description": "Run the code as a script of its own and return immediately with a job id instead of waiting for it. Use it for anything that outlasts a couple of minutes. It starts where you stand but in a new interpreter, without your variables. You are woken when it exits, with its status and the tail of its log." },
                    "restart": { "type": "boolean", "description": "Start from a new interpreter first, dropping everything earlier cells declared. For when what it holds is in your way — it is never needed just because the last call threw." },
                },
                "required": ["code"],
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
        description: "Ask to be woken later, then carry on and finish this turn. When one of `events` is \
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
        description: "Put a decision to the person you are working for, then **end your turn**. You are not \
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
    ToolSpec {
        name: "Report",
        description: "Hand whoever launched you an interim report, on purpose, without waiting \
             for the run itself to end. Reach for this at a natural checkpoint — \"phase 1 done, \
             here's what's next\" — when a person or another agent started you and will be told \
             when you finish: this reaches them right now, even while you still have an Await \
             pending, a background job running, or more work queued. It does not end anything by \
             itself and it is not a question — nobody is expected to answer it. Keep going, or end \
             your turn exactly as you would without it.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The report itself — what you would tell whoever started you if they asked how it's going." },
                },
                "required": ["text"],
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
const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "dist",
    ".build",
    "__pycache__",
];
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
    pub name: String,
    pub description: String,
    /// The JSON Schema `object` describing the call's arguments.
    pub schema: Value,
}

/// Every tool an `harness:adi` turn of an agent with these switches can call, exactly as declared
/// to the model.
///
/// Read off the same [`TOOLS`] table [`super::adi_loop`] builds its provider payloads from, through
/// the same [`offered`] filter, so a tool added there appears here without anybody remembering to. Anything that renders this list
/// for a person is then showing the inventory the model has rather than a second copy of it that
/// can fall behind.
#[must_use]
pub fn declarations(code: CodeTools) -> Vec<ToolDeclaration> {
    declarations_with(code, &[])
}

/// [`declarations`], for an agent with these MCP servers attached.
///
/// The servers matter to exactly one kind of agent: a code-only one, whose single tool's
/// description is where it learns they are there (see [`described`]). Every caller that declares
/// tools to a model comes through here — the adi loop, the MCP server, the simulator — so what
/// that description says cannot differ between them.
#[must_use]
pub fn declarations_with(code: CodeTools, servers: &[String]) -> Vec<ToolDeclaration> {
    offered(code)
        .map(|t| ToolDeclaration {
            name: t.name.to_string(),
            description: described(t, code, servers),
            schema: (t.schema)(),
        })
        .collect()
}

/// What a tool is told to the model as, for an agent with these switches.
///
/// Its own description — except for a code-only agent's one tool, which also has to say
/// everything the tools it replaced would have said, because nothing else will.
fn described(spec: &ToolSpec, code: CodeTools, servers: &[String]) -> String {
    #[cfg(unix)]
    if code.only {
        let language = match spec.name {
            "PyCode" => Some(Language::Python),
            "TsCode" => Some(Language::TypeScript),
            _ => None,
        };
        if let Some(language) = language {
            return crate::backends::code_only::describe(spec.description, language, servers);
        }
    }
    let _ = (code, servers);
    spec.description.to_string()
}

impl<'a> Ctx<'a> {
    /// The context one conversation's tool calls run in.
    ///
    /// Built here rather than at each call site because the pieces are not interchangeable: the
    /// shell is keyed by the *conversation*, so a second construction that keyed it by the turn
    /// would silently give a run a fresh shell — no `cd`, no exports — and the tools would keep
    /// working while quietly meaning something else. The adi loop and the simulator both come
    /// through here, which is what makes a call made by a person the same call the model makes.
    #[allow(clippy::too_many_arguments)] // each is a different fact about the conversation, and
    // a struct to carry them here would be this struct
    pub(crate) fn for_conversation(
        agent: &'a str,
        unattended: bool,
        code: CodeTools,
        mcp: Hub,
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
            code,
            mcp,
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
    // Only what this agent has. A code tool that is switched off, and every tool but the code
    // tool for a code-only agent, is answered as any name it was never given is — which is what
    // these are to it.
    let absent = || {
        format!(
            "no tool named {name} — the tools you have are: {}",
            offered(ctx.code).map(|t| t.name).collect::<Vec<_>>().join(", ")
        )
    };
    if !ctx.code.offers(name) {
        return Err(absent());
    }
    match name {
        #[cfg(unix)]
        "PyCode" => cell(Language::Python, input, ctx),
        #[cfg(unix)]
        "TsCode" => cell(Language::TypeScript, input, ctx),
        _ => plain(name, input, ctx).unwrap_or_else(|| Err(absent())),
    }
}

/// Run one tool call that code made — a function of a code-only agent's cell (see
/// [`crate::backends::code_only`]).
///
/// The same tools, by the same code, with two differences. The code tools are not among them:
/// the interpreter is busy running the cell that is asking. And nothing called from a cell may
/// outlast it, so a command started through `Bash` is given what is left of the cell's own
/// `deadline` rather than a deadline of its own.
///
/// # Errors
/// The tool's own answer when it fails, exactly as [`execute`] hands it to a model — here it
/// becomes the exception the function raises.
#[cfg(unix)]
pub(crate) fn execute_inner(
    name: &str,
    input: &Value,
    ctx: &Ctx<'_>,
    deadline: std::time::Instant,
) -> std::result::Result<String, String> {
    if is_code_tool(name) {
        return Err(format!(
            "{name} cannot be called from code: it is the tool your code is already running in"
        ));
    }
    let answered = if name == "Bash" && !flag(input, "background") {
        let left = u64::try_from(
            deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_millis(),
        )
        .unwrap_or(u64::MAX)
        .max(1);
        let asked = arg_u64(input, "timeout_ms").unwrap_or(DEFAULT_TIMEOUT_MS);
        let mut input = input.clone();
        if let Some(arguments) = input.as_object_mut() {
            arguments.insert("timeout_ms".to_string(), json!(asked.min(left)));
        }
        plain(name, &input, ctx)
    } else {
        plain(name, input, ctx)
    };
    answered.unwrap_or_else(|| {
        Err(format!(
            "no tool named {name} — the functions there are: {}",
            inner().map(|t| t.name).collect::<Vec<_>>().join(", ")
        ))
    })
}

/// Run a tool that is not a code tool, or `None` when `name` is not one of them.
fn plain(name: &str, input: &Value, ctx: &Ctx<'_>) -> Option<std::result::Result<String, String>> {
    Some(match name {
        "Read" => read(input, ctx.cwd),
        "Write" => write(input, ctx.cwd),
        "Edit" => edit(input, ctx.cwd),
        "Bash" => bash(input, ctx),
        "Glob" => glob(input, ctx.cwd),
        "Grep" => grep(input, ctx.cwd),
        "Await" => await_wake(input, ctx),
        "Ask" => ask(input, ctx),
        "Report" => report(input, ctx),
        _ => return None,
    })
}

fn read(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let path = resolve(arg_str(input, "path")?, cwd);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;

    // Saturating rather than casting: a model that asks for line 2^40 of a file gets the end of
    // it, not a number that wrapped into something else entirely.
    let offset =
        usize::try_from(arg_u64(input, "offset").unwrap_or(1).max(1)).unwrap_or(usize::MAX);
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
    std::fs::write(&path, content)
        .map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
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
    if flag(input, "background") {
        let note = format!("You started this in the background:\n\n{command}");
        return background(command, note, ctx);
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
        return Ok(with_note(
            &truncate(&format!("(exit {code})\n{text}")),
            moved,
        ));
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
///
/// `note` is what the wake says was started — the caller's to word, because what the model wrote
/// is not always the command that runs: a background `PyCode` is a script on disk, and telling the
/// model it started `python3 … run …` would describe the wrapper instead of its work.
fn background(command: &str, note: String, ctx: &Ctx<'_>) -> std::result::Result<String, String> {
    let start = ctx.shell.start_dir(ctx.cwd);
    let job = jobs::start(ctx.agent_dir, ctx.conv, &ctx.shell, &start, command)
        .map_err(|e| format!("couldn't start the job: {e}"))?;

    let request = Request {
        note,
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

/// How much of a background cell's source travels in the note that wakes its conversation. The
/// note is replayed with every later turn, and the whole script is on disk beside the job's log.
#[cfg(unix)]
const WAKE_CODE_CHARS: usize = 2_000;

/// Run a cell in the conversation's interpreter for `language` — see
/// [`crate::backends::interpreter`].
///
/// Where `Bash` reports a non-zero exit as a result, this reports an escaped exception as a
/// *failed* one, and the difference is deliberate. A command's status is routinely an answer — a
/// `grep` that found nothing — so the shell tool cannot call it a failure. Here that case has a
/// spelling of its own (`check=False`, `{ check: false }`), which leaves an exception meaning only
/// one thing: the cell did not do what it was written to do.
#[cfg(unix)]
fn cell(language: Language, input: &Value, ctx: &Ctx<'_>) -> std::result::Result<String, String> {
    use crate::backends::interpreter::{Ended, Interpreter};

    let code = arg_str(input, "code")?;
    let interpreter = Interpreter::new(language, ctx.agent_dir, ctx.conv);
    if flag(input, "background") {
        // A code-only agent's script is told which names are the tools it has in a cell and
        // not here, so that reaching for one explains itself.
        let absent: Vec<&str> = if ctx.code.only {
            inner().map(|spec| spec.name).collect()
        } else {
            Vec::new()
        };
        let command = interpreter.job_command(code, &absent)?;
        let shown: String = code.chars().take(WAKE_CODE_CHARS).collect();
        let more = if shown.len() < code.len() { "\n[…]" } else { "" };
        let note = format!(
            "You started this {} in the background:\n\n{shown}{more}",
            language.name()
        );
        return background(&command, note, ctx);
    }

    let timeout = arg_u64(input, "timeout_ms").unwrap_or(DEFAULT_TIMEOUT_MS);
    let start = ctx.shell.start_dir(ctx.cwd);
    let restart = flag(input, "restart");
    let run = |offered: Option<&Value>| {
        interpreter.run(&ctx.shell, ctx.cwd, code, timeout, MAX_OUTPUT, restart, offered)
    };
    let ran = if ctx.code.only {
        // The cell's functions are this agent's other tools, answered from here for as long as
        // the cell runs.
        crate::backends::code_only::serving(ctx, timeout, |offered| run(Some(offered)))??
    } else {
        run(None)?
    };
    let moved = ctx
        .shell
        .moved_from(&start)
        .map(|ended| format!("\n(the shell is now in {})", ended.display()));

    let printed = ran.output.trim_end();
    let mut text = String::new();
    if ran.replaced {
        // First, not last: everything below it was computed without the variables the model
        // believes it has, and a `NameError` read before this line reads as its own mistake.
        text.push_str(
            "(this is a new interpreter: the one this conversation had is gone, and every \
             variable, import and definition it held went with it. Files, the directory and \
             exported variables are as you left them — rebuild what you still need.)\n\n",
        );
    }
    text.push_str(if printed.is_empty() {
        "(no output)"
    } else {
        printed
    });
    match ran.ended {
        Ended::Completed => Ok(with_note(&text, moved)),
        Ended::Raised => Err(with_note(&text, moved)),
        Ended::TimedOut => {
            // What was done about the deadline is the one thing the two interpreters do
            // differently, and the model is about to decide whether to run the cell again.
            let stopped = match language {
                Language::Python => "was interrupted where the traceback shows",
                Language::TypeScript => "the command it was waiting on was stopped",
            };
            let _ = write!(
                text,
                "\n\n(the code was still running after {timeout}ms and {stopped}. The \
                 interpreter and your variables are intact. Set `background` to let it run past \
                 this call, or raise timeout_ms.)"
            );
            Err(with_note(&text, moved))
        }
        Ended::Interrupted => {
            text.push_str("\n\n(the code was interrupted before it finished.)");
            Err(with_note(&text, moved))
        }
    }
}

/// A boolean argument that is absent, or anything but `true`, is off.
fn flag(input: &Value, key: &str) -> bool {
    input.get(key).and_then(Value::as_bool).unwrap_or(false)
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
    let listed: Vec<String> = hits.iter().map(|(_, p)| p.display().to_string()).collect();
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
    let check = input
        .get("check")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(check) = &check {
        refuse_exited_job(check, ctx)?;
    }
    let req = Request {
        note: arg_str(input, "note")?.to_string(),
        events: string_list(input, "events"),
        when: string_map(input, "when")?,
        after_seconds: arg_u64(input, "after_seconds"),
        every_seconds: arg_u64(input, "every_seconds"),
        check,
        expires_in_seconds: arg_u64(input, "expires_in_seconds"),
        cwd: ctx.cwd.display().to_string(),
        baseline_seq: None,
    };
    let registered =
        awaits::register(&ctx.awaits, ctx.agent, ctx.conv, &req).map_err(|e| e.to_string())?;
    Ok(format!(
        "registered await {} — waking {}. Finish this turn; you will be woken with your note.",
        registered.id,
        registered.describe()
    ))
}

/// Refuse a check that is still naming a job [`jobs::start`] has already finished (ADI-MONO-129
/// (3)).
///
/// `background` already registered this job's own wake, so a hand-written check watching the same
/// job by id has nothing left to report once that job is gone — and, worse, if it is wrong the way
/// the bug behind this was, a check that can never agree parks the run until the await's own
/// `expires_at`, which may be days off. Caught here, before the await is ever stored, rather than
/// left for the garbage collection [`awaits::tick`] does for one that slips through some other way
/// (an `update`, or a check copied from one await into another).
fn refuse_exited_job(check: &str, ctx: &Ctx<'_>) -> std::result::Result<(), String> {
    let Some(job) = jobs::referenced_in(ctx.agent_dir, ctx.conv, check) else {
        return Ok(());
    };
    if !jobs::has_exited(&job) {
        return Ok(());
    }
    Err(format!(
        "job {} has already exited, and a background job wakes you itself when it ends — a check \
         still watching it by hand is redundant now and may never be asked again. Read its log \
         directly if you need what it said: {}",
        job.id,
        job.log.display()
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
    let check = input
        .get("check")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(check) = check.as_deref().filter(|c| !c.is_empty()) {
        refuse_exited_job(check, ctx)?;
    }
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
        check,
        expires_in_seconds: arg_u64(input, "expires_in_seconds"),
    };
    let changed =
        awaits::update(&ctx.awaits, ctx.agent, ctx.conv, id, &change).map_err(|e| e.to_string())?;
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
    // The write and the event it announces are made atomic here (ADI-MONO-149): `ask` already
    // opens its own transaction (the pending-ask check and the insert have to be atomic on their
    // own), so this goes through `ask_with_outbox` — which writes the event's record into that
    // same transaction, right before `ask`'s own commit — rather than `SessionStore::with_outbox`
    // (which would try to open a second, outer transaction and fail). Relaying afterward is
    // best-effort: a crash in between can only delay the event reaching the shared log, never
    // lose it. This runs outside `Agents`'s own `with_event` choke point (there is no `Agents`
    // handle here, only this store and `ctx.awaits`'s `Config`), but through the same primitives.
    let asked = ctx
        .sessions
        .ask_with_outbox(ctx.agent, ctx.conv, &req, |asked| {
            crate::events::AgentQuestionAsked {
                agent: ctx.agent.to_string(),
                conv: ctx.conv.to_string(),
                run_id: ctx.conv.to_string(),
                ask: asked.id.clone(),
                question: asked.headline(),
            }
            .to_record()
            .map(Some)
            .map_err(|e| crate::Error::Session(format!("couldn't build an event record: {e}")))
        })
        .map_err(|e| e.to_string())?;
    let _ = ctx.sessions.relay_pending_events(ctx.awaits.config());

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

/// Tell whoever launched this run something, on purpose, without waiting for the run to end.
///
/// Announced on the bus for the same reason [`ask`] is: this crate has no idea who is watching,
/// and [`awaits::follow_up`]'s registered wake is what turns it into somebody's notification.
/// Nothing here touches an await, the queue, or the transcript — a report changes nothing about
/// whether the conversation is still going, only who has been told about it. There is no write to
/// make this atomic with, but it still goes through the same outbox [`ask`] does (ADI-MONO-149):
/// the event is a committed local row before this returns, which a direct publish never was.
fn report(input: &Value, ctx: &Ctx<'_>) -> std::result::Result<String, String> {
    let text = arg_str(input, "text")?;
    let _ = ctx.sessions.with_outbox(|| {
        let record = crate::events::AgentRunReported {
            agent: ctx.agent.to_string(),
            run_id: ctx.conv.to_string(),
            report: text.trim().to_string(),
        }
        .to_record()
        .map_err(|e| crate::Error::Session(format!("couldn't build an event record: {e}")))?;
        Ok(((), Some(record)))
    });
    let _ = ctx.sessions.relay_pending_events(ctx.awaits.config());
    Ok(
        "reported — whoever is watching this run (if anyone registered a wake for it) has been \
         told. Carry on, or end your turn."
            .to_string(),
    )
}

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
    let object = value.as_object().ok_or_else(|| {
        format!("`{key}` is an object of field names to the values they must have")
    })?;
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
                let after = if p.get(2) == Some(&'/') {
                    &p[3..]
                } else {
                    &p[2..]
                };
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
    use crate::AgentRunFinished;
    use adi_events::Event;

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
            code: CodeTools::BOTH,
            mcp: Hub::default(),
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
                code: CodeTools::BOTH,
                mcp: Hub::default(),
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

        let all =
            json!({"path": "f.txt", "old_string": "a", "new_string": "b", "replace_all": true});
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
        let windowed =
            read(&json!({"path": "f.txt", "offset": 2, "limit": 1}), &dir).expect("read");
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

    /// The code tools are switches on the agent, and one that is off is not there: not in what the
    /// model is declared, and answered like any name it was never given if it is called anyway —
    /// with the list of what the agent does have, which does not mention it either.
    #[cfg(unix)]
    #[test]
    fn a_code_tool_exists_only_for_an_agent_that_switched_it_on() {
        let names = |code: CodeTools| -> Vec<String> {
            declarations(code).into_iter().map(|tool| tool.name).collect()
        };
        let neither = names(CodeTools::default());
        assert!(neither.contains(&"Bash".to_string()), "{neither:?}");
        assert!(!neither.iter().any(|name| name == "PyCode" || name == "TsCode"), "{neither:?}");
        let python = names(CodeTools { python: true, ..CodeTools::default() });
        assert!(python.contains(&"PyCode".to_string()) && !python.contains(&"TsCode".to_string()), "{python:?}");
        let both = names(CodeTools::BOTH);
        assert!(both.contains(&"PyCode".to_string()) && both.contains(&"TsCode".to_string()), "{both:?}");

        let dir = scratch("code-off");
        let mut ctx = ctx_in(&dir, "code-off");
        ctx.code = CodeTools { typescript: true, ..CodeTools::default() };
        let refused = execute("PyCode", &json!({ "code": "1" }), &ctx).expect_err("it has no PyCode");
        assert!(refused.starts_with("no tool named PyCode"), "{refused}");
        assert!(refused.contains("TsCode"), "what it does have is named: {refused}");
        assert!(!refused.contains("PyCode,"), "and what it does not is not: {refused}");
        assert!(
            !dir.join("conv-1.py-kernel").exists(),
            "nothing was started for a tool the agent does not have"
        );

        let allowed = execute("TsCode", &json!({ "code": "1 + 1;" }), &ctx).expect("it has TsCode");
        assert_eq!(allowed, "2");
        crate::backends::interpreter::Interpreter::new(Language::TypeScript, ctx.agent_dir, ctx.conv).stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `TsCode` call as the model meets it: the last expression comes back, a thrown error is a
    /// failed result naming the line, the next call still has what the first declared — and a
    /// deadline says which of the two things a deadline can mean here happened.
    #[cfg(unix)]
    #[test]
    fn tscode_keeps_what_it_declared_fails_the_call_that_throws_and_reports_a_deadline() {
        let dir = scratch("tscode");
        std::fs::create_dir_all(dir.join("workspaces")).expect("mkdir");
        let ctx = ctx_in(&dir, "tscode");
        let typescript = crate::backends::interpreter::Interpreter::new(Language::TypeScript, ctx.agent_dir, ctx.conv);

        let quiet = execute("TsCode", &json!({ "code": "const rows: number[] = [1, 2, 3];" }), &ctx).expect("run");
        assert_eq!(quiet, "(no output)");
        let shown = execute("TsCode", &json!({ "code": "rows.reduce((a, b) => a + b, 0);" }), &ctx).expect("run");
        assert_eq!(shown, "6");

        let thrown = execute("TsCode", &json!({ "code": "console.log('so far');\n(rows as any).nope.x;" }), &ctx)
            .expect_err("an exception fails the call");
        assert!(thrown.starts_with("so far\n"), "{thrown}");
        assert!(thrown.contains("TypeError"), "{thrown}");
        assert!(thrown.contains("at <cell 3>: rows.nope.x;"), "{thrown}");

        let moved = execute("TsCode", &json!({ "code": "process.chdir('workspaces');" }), &ctx).expect("run");
        assert!(moved.contains("the shell is now in"), "{moved}");
        let after = bash(&json!({ "command": "basename \"$(pwd -P)\"" }), &ctx).expect("run");
        assert!(after.starts_with("workspaces"), "Bash stands where TypeScript moved: {after}");

        let late = execute("TsCode", &json!({ "code": "await sh('sleep 60');", "timeout_ms": 300 }), &ctx)
            .expect_err("a timeout fails the call");
        assert!(late.contains("still running after 300ms"), "{late}");
        assert!(late.contains("the command it was waiting on was stopped"), "{late}");
        assert!(late.contains("variables are intact"), "{late}");
        assert_eq!(execute("TsCode", &json!({ "code": "rows.length;" }), &ctx).expect("run"), "3");

        typescript.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A background `TsCode` is a job like any other, and its wake quotes the code.
    #[cfg(unix)]
    #[test]
    fn a_background_tscode_starts_a_job_and_the_wake_quotes_the_code() {
        let dir = scratch("tscode-bg");
        let ctx = ctx_in(&dir, "tscode-bg");

        let said = execute(
            "TsCode",
            &json!({ "code": "console.log('from the job');", "background": true }),
            &ctx,
        )
        .expect("a job starts");
        assert!(said.contains("in the background"), "{said}");

        let pending = ctx.awaits.for_conversation("watcher", "conv-1");
        assert_eq!(pending.len(), 1, "exactly one wake: {pending:?}");
        assert!(pending[0].note.contains("this TypeScript in the background"), "{:?}", pending[0]);
        assert!(pending[0].note.contains("console.log('from the job');"), "{:?}", pending[0]);

        let log = said
            .lines()
            .find_map(|line| line.trim().strip_prefix("log: "))
            .expect("the log path is in the reply");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !std::fs::read_to_string(log).is_ok_and(|text| text.contains("from the job"))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(std::fs::read_to_string(log).expect("log").trim(), "from the job");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `PyCode` call as the model meets it: what was printed comes back, an escaped exception is
    /// a *failed* result with its traceback, and the next call still has the first one's variables.
    #[cfg(unix)]
    #[test]
    fn pycode_keeps_its_variables_and_fails_the_call_that_raises() {
        let dir = scratch("pycode");
        let ctx = ctx_in(&dir, "pycode");
        let python = crate::backends::interpreter::Interpreter::new(Language::Python, ctx.agent_dir, ctx.conv);

        let quiet = execute("PyCode", &json!({ "code": "rows = [1, 2, 3]" }), &ctx).expect("run");
        assert_eq!(quiet, "(no output)");

        let shown = execute("PyCode", &json!({ "code": "sum(rows)" }), &ctx).expect("run");
        assert_eq!(shown, "6", "the variable survived, and the expression is echoed");

        let raised = execute("PyCode", &json!({ "code": "print('so far')\nrows[9]" }), &ctx)
            .expect_err("an exception fails the call");
        assert!(raised.starts_with("so far\n"), "{raised}");
        assert!(raised.contains("IndexError"), "{raised}");

        let after = execute("PyCode", &json!({ "code": "len(rows)" }), &ctx).expect("run");
        assert_eq!(after, "3", "the exception cost the interpreter nothing");

        python.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The directory is the conversation's, not the tool's: Python moving it is reported the way a
    /// `cd` is, and `Bash` is standing there afterwards.
    #[cfg(unix)]
    #[test]
    fn pycode_moves_the_same_shell_bash_uses_and_says_so() {
        let dir = scratch("pycode-cd");
        std::fs::create_dir_all(dir.join("workspaces")).expect("mkdir");
        let inner = std::fs::canonicalize(dir.join("workspaces")).expect("canonicalize");
        let inner = inner.to_str().expect("utf8");
        let ctx = ctx_in(&dir, "pycode-cd");
        let python = crate::backends::interpreter::Interpreter::new(Language::Python, ctx.agent_dir, ctx.conv);

        let stayed = execute("PyCode", &json!({ "code": "1" }), &ctx).expect("run");
        assert!(!stayed.contains("the shell is now in"), "{stayed}");

        let moved = execute("PyCode", &json!({ "code": "os.chdir('workspaces')" }), &ctx).expect("run");
        assert!(moved.contains("the shell is now in"), "{moved}");
        assert!(moved.contains(inner), "{moved}");

        let after = bash(&json!({ "command": "pwd -P" }), &ctx).expect("run");
        assert!(after.contains(inner), "{after}");

        python.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A deadline is a failed call that says what survived it — which is everything but the cell.
    #[cfg(unix)]
    #[test]
    fn pycode_past_its_deadline_says_the_variables_are_intact() {
        let dir = scratch("pycode-deadline");
        let ctx = ctx_in(&dir, "pycode-deadline");
        let python = crate::backends::interpreter::Interpreter::new(Language::Python, ctx.agent_dir, ctx.conv);

        let said = execute(
            "PyCode",
            &json!({ "code": "import time\ntime.sleep(60)", "timeout_ms": 300 }),
            &ctx,
        )
        .expect_err("a timeout fails the call");
        assert!(said.contains("still running after 300ms"), "{said}");
        assert!(said.contains("variables are intact"), "{said}");
        assert!(said.contains("background"), "and what to do instead: {said}");

        python.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A background cell is a job like any other: started and watched in one act, and the wake
    /// reminds the run of the *code* it wrote rather than of the wrapper that runs it.
    #[cfg(unix)]
    #[test]
    fn a_background_pycode_starts_a_job_and_the_wake_quotes_the_code() {
        let dir = scratch("pycode-bg");
        let ctx = ctx_in(&dir, "pycode-bg");

        let said = execute(
            "PyCode",
            &json!({ "code": "print('from the job')", "background": true }),
            &ctx,
        )
        .expect("a job starts");
        assert!(said.contains("in the background"), "{said}");
        assert!(said.contains("job-"), "{said}");

        let pending = ctx.awaits.for_conversation("watcher", "conv-1");
        assert_eq!(pending.len(), 1, "exactly one wake: {pending:?}");
        assert!(
            pending[0].note.contains("print('from the job')"),
            "{:?}",
            pending[0]
        );
        assert!(
            !pending[0].note.contains("pycode.py"),
            "the wrapper is not what the model started: {:?}",
            pending[0]
        );

        // The job really ran the cell: its log ends up holding what the code printed.
        let log = said
            .lines()
            .find_map(|line| line.trim().strip_prefix("log: "))
            .expect("the log path is in the reply");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !std::fs::read_to_string(log).is_ok_and(|text| text.contains("from the job"))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(
            std::fs::read_to_string(log).expect("log").trim(),
            "from the job"
        );

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
        let out = bash(
            &json!({"command": "echo out; exit 3"}),
            &ctx_in(&dir, "bash"),
        )
        .expect("nonzero is a result");
        assert!(out.contains("exit 3"), "{out}");
        assert!(out.contains("out"), "{out}");
    }

    #[test]
    fn a_command_that_never_finishes_is_killed_and_says_so() {
        let dir = scratch("bash-timeout");
        let err = bash(
            &json!({"command": "sleep 5", "timeout_ms": 200}),
            &ctx_in(&dir, "bash"),
        )
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
        assert!(
            err.contains("Await"),
            "the wake tool is advertised too: {err}"
        );
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

    /// ADI-MONO-129 (3): `background` already arranged a wake for a job's own ending — a hand-written
    /// check still naming that job after it has exited is refused, with the log path to read instead.
    #[cfg(unix)]
    #[test]
    fn await_refuses_a_check_still_watching_a_job_that_has_already_exited() {
        let dir = scratch("await-dead-job");
        let ctx = ctx_in(&dir, "await-dead-job");

        let job = jobs::start(ctx.agent_dir, ctx.conv, &ctx.shell, &dir, "exit 0").expect("start");
        for _ in 0..100 {
            if job.exit.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(job.exit.exists(), "the job settles");

        let err = await_wake(
            &json!({
                "note": "watching it myself",
                "every_seconds": 60,
                "check": format!("tail -c 100 {}", job.log.display()),
            }),
            &ctx,
        )
        .expect_err("refused — the job already has its own wake");
        assert!(err.contains(&job.id), "{err}");
        assert!(err.contains("already exited"), "{err}");

        // A live job is a different matter: it has not reported anything yet, so there is nothing
        // redundant about asking after it.
        let running = jobs::start(ctx.agent_dir, ctx.conv, &ctx.shell, &dir, "sleep 30").expect("start");
        await_wake(
            &json!({
                "note": "watching it myself",
                "every_seconds": 60,
                "check": format!("tail -c 100 {}", running.log.display()),
            }),
            &ctx,
        )
        .expect("a running job's own check is not refused");
        let _ = Command::new("kill")
            .arg("-9")
            .arg(running.pid.to_string())
            .status();

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
                "events": [AgentRunFinished::NAME],
                "when": { "run_id": "r-42" },
            }),
            &ctx,
        )
        .expect("register");
        assert!(
            registered.contains("run_id=r-42"),
            "the filter is in what it reads: {registered}"
        );

        let id = ctx.awaits.for_conversation("watcher", "conv-1")[0]
            .id
            .clone();

        let changed = await_wake(
            &json!({ "update": id, "note": "why I am waiting", "expires_in_seconds": 600 }),
            &ctx,
        )
        .expect("update");
        assert!(changed.contains(&id), "the id survives a change: {changed}");
        let pending = ctx.awaits.for_conversation("watcher", "conv-1");
        assert_eq!(pending.len(), 1, "changed in place, not registered again");
        assert_eq!(pending[0].note, "why I am waiting");
        assert_eq!(pending[0].events, vec![AgentRunFinished::NAME.to_string()]);
        assert!(
            pending[0].expiry_wakes,
            "a deadline it named is one it hears about"
        );

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
        await_wake(
            &json!({ "note": "waiting", "events": ["adi.tasks.*"] }),
            &ctx,
        )
        .expect("register");
        let id = ctx.awaits.for_conversation("watcher", "conv-1")[0]
            .id
            .clone();

        let err =
            await_wake(&json!({ "update": "w-nope", "note": "x" }), &ctx).expect_err("unknown");
        assert!(err.contains(&id), "the real one is named: {err}");
        // `ignore` reports per id rather than failing the call, so a run dropping several is not
        // stopped by one that has already fired.
        let mixed =
            await_wake(&json!({ "ignore": ["w-nope"] }), &ctx).expect("reported, not failed");
        assert!(mixed.contains(&id), "{mixed}");
        assert!(mixed.contains("1 await(s) still pending"), "{mixed}");

        let _ = std::fs::remove_dir_all(&dir);
    }

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

    /// ADI-MONO-149: a crash in the gap between `ask`'s own commit and the best-effort relay that
    /// follows it must not lose the `adi.agents.question.asked` event — only delay it. Simulated
    /// the same way `adi_tasks` tests the same guarantee: the shared log's own directory is blocked
    /// with a plain file, so `relay_pending_events` fails exactly as a crash would have stopped it
    /// from ever running, while the question row — and the event's record, in the SAME transaction
    /// — are already durable in `sessions.db`.
    #[test]
    fn a_crash_between_commit_and_relay_still_delivers_the_question_asked_event() {
        let chat = Conversation::open("crash-before-relay");
        let ctx = chat.ctx();
        let log = adi_events::Events::with_config(ctx.awaits.config().clone());
        std::fs::create_dir_all(ctx.awaits.config().root()).expect("create root");
        std::fs::write(log.dir(), "not a directory").expect("block the shared log");

        ask(
            &json!({
                "note": "ship now or wait",
                "questions": [question("ship now, or wait for review?")],
            }),
            &ctx,
        )
        .expect("the ask still succeeds — the shared log being unreachable never blocks it");
        let pending = ctx
            .sessions
            .pending_question("watcher", &chat.conv)
            .expect("the question is already durable in sessions.db");

        assert!(
            ctx.sessions
                .relay_pending_events(ctx.awaits.config())
                .is_err(),
            "relaying while the log is blocked is reported, not swallowed"
        );

        std::fs::remove_file(log.dir()).expect("unblock the shared log");
        assert_eq!(
            ctx.sessions
                .relay_pending_events(ctx.awaits.config())
                .expect("relay now that the log is reachable"),
            1,
            "the event that crashed before relaying is still exactly one pending row"
        );
        let caught_up = log.catch_up("sub", 10).expect("catch up");
        assert_eq!(caught_up.len(), 1);
        assert_eq!(caught_up[0].record.name, "adi.agents.question.asked");
        let payload: serde_json::Value =
            serde_json::from_str(&caught_up[0].record.payload).expect("payload");
        assert_eq!(payload["ask"], pending.id);
        assert!(
            payload["question"]
                .as_str()
                .unwrap_or_default()
                .contains("ship now, or wait for review?"),
            "{payload}"
        );
    }

    /// ADI-MONO-149: once a relay has forwarded a row it deletes it locally, so a second relay —
    /// the next write's own best-effort call, or the app's periodic sweep — finds nothing left and
    /// the event is not appended to the shared log twice.
    #[test]
    fn re_relaying_after_a_successful_relay_does_not_double_deliver() {
        let chat = Conversation::open("no-double-delivery");
        let ctx = chat.ctx();
        let log = adi_events::Events::with_config(ctx.awaits.config().clone());

        ask(
            &json!({ "questions": [question("one more time?")] }),
            &ctx,
        )
        .expect("asked");
        // `ask` already relays best-effort on success; this call finds nothing left pending.
        assert_eq!(
            ctx.sessions
                .relay_pending_events(ctx.awaits.config())
                .expect("relay"),
            0,
            "the one event was already relayed by `ask` itself"
        );
        let caught_up = log.catch_up("sub", 10).expect("catch up");
        assert_eq!(caught_up.len(), 1, "the fact appears exactly once, not twice");
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

    /// ADI-MONO-101's explicit signal: a run can hand over an interim report on purpose, and that
    /// is announced on the bus exactly like an `Ask` is — under its own name, so a launcher's wake
    /// can tell it apart from the run actually ending.
    #[test]
    fn report_announces_itself_on_the_bus_and_touches_nothing_else() {
        let chat = Conversation::open("report");
        let ctx = chat.ctx();

        let reply = report(&json!({ "text": "phase 1 done, starting phase 2" }), &ctx)
            .expect("reported");
        assert!(reply.contains("reported"), "{reply}");

        let events = adi_events::Events::with_config(ctx.awaits.config().clone())
            .catch_up("test", 10)
            .expect("catch up");
        let published = events
            .iter()
            .find(|e| e.record.name == crate::events::RUN_REPORTED)
            .expect("the report was published");
        let payload: Value =
            serde_json::from_str(&published.record.payload).expect("payload decodes");
        assert_eq!(payload["agent"], "watcher");
        assert_eq!(payload["run_id"], chat.conv);
        assert_eq!(payload["report"], "phase 1 done, starting phase 2");

        // A report is a side channel: it settles no await, queues nothing, and asks nobody anything.
        assert!(ctx.awaits.for_conversation("watcher", &chat.conv).is_empty());
        assert!(
            ctx.sessions
                .pending_question("watcher", &chat.conv)
                .is_none()
        );
    }

    /// A code-only agent's context: the scratch stores of [`ctx_in`], the one code tool, and
    /// whatever MCP servers the test attaches.
    #[cfg(unix)]
    fn only_in<'a>(cwd: &'a Path, tag: &str, code: CodeTools, mcp: Hub) -> Ctx<'a> {
        Ctx {
            code,
            mcp,
            ..ctx_in(cwd, tag)
        }
    }

    /// The mode in one test: such an agent is declared one tool, calling any other is calling a
    /// name it was never given — and from inside the one it has, the others are functions that do
    /// exactly what the tools do, on the files the code itself can see.
    #[cfg(unix)]
    #[test]
    fn a_code_only_agent_has_one_tool_and_calls_the_others_from_inside_it() {
        let dir = scratch("only-ts");
        let ctx = only_in(&dir, "only-ts", CodeTools::ONLY_TYPESCRIPT, Hub::default());
        let typescript = crate::backends::interpreter::Interpreter::new(Language::TypeScript, ctx.agent_dir, ctx.conv);

        let declared = declarations_with(ctx.code, &ctx.mcp.ids());
        let names: Vec<&str> = declared.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names, ["TsCode"], "one tool, and it is the interpreter");
        assert!(
            declared[0].description.contains("This is the only tool you have")
                && declared[0].description.contains("`await Edit({ new_string, old_string, path, replace_all? })`"),
            "which says what the rest became: {}",
            declared[0].description
        );

        for gone in ["Read", "Bash", "Await", "PyCode"] {
            let refused = execute(gone, &json!({ "path": "x", "command": "true", "code": "1" }), &ctx)
                .expect_err("it is not a tool this agent has");
            assert_eq!(refused, format!("no tool named {gone} — the tools you have are: TsCode"));
        }

        let ran = execute(
            "TsCode",
            &json!({ "code": "
                const wrote = await Write({ path: 'notes/a.txt', content: 'one\\ntwo\\n' });
                const edited = await Edit({ path: 'notes/a.txt', old_string: 'two', new_string: 'three' });
                const text = await Read({ path: 'notes/a.txt' });
                const found = await Glob({ pattern: '**/*.txt' });
                const hits = await Grep({ pattern: 'thr+ee' });
                console.log(text);
                console.log(fs.readFileSync('notes/a.txt', 'utf8') === 'one\\nthree\\n', found.endsWith('notes/a.txt'), hits.includes('a.txt:2: three'));
            " }),
            &ctx,
        )
        .expect("the cell runs");
        assert_eq!(ran, "one\nthree\ntrue true true", "assigned, so nothing was shown but what was printed");

        // A call on a line of its own shows what the tool answered, wherever in the cell it
        // stands and as the tool said it — a file's text is not shown quoted and escaped.
        let shown = execute(
            "TsCode",
            &json!({ "code": "await Write({ path: 'notes/b.txt', content: 'x' });\nconst kept = 1;\nawait Read({ path: 'notes/a.txt' });" }),
            &ctx,
        )
        .expect("a cell of bare calls");
        let lines: Vec<&str> = shown.lines().collect();
        assert_eq!(lines.len(), 3, "{shown}");
        assert!(lines[0].starts_with("wrote ") && lines[0].ends_with("notes/b.txt (1 bytes)"), "{shown}");
        assert_eq!(&lines[1..], ["one", "three"], "{shown}");

        // What an earlier cell kept is still there, and so are the tools: the door is opened for
        // each cell, not once.
        let again = execute(
            "TsCode",
            &json!({ "code": "const now = await Read({ path: 'notes/a.txt' }); now === text;" }),
            &ctx,
        )
        .expect("a second cell");
        assert_eq!(again, "true");

        // A tool that fails is an exception the code may catch…
        let caught = execute(
            "TsCode",
            &json!({ "code": "try { await Read({ path: 'no-such-file' }); } catch (error) { console.log(error.name, '|', error.tool, '|', error.message); }" }),
            &ctx,
        )
        .expect("caught, so the cell succeeds");
        assert!(caught.starts_with("ToolError | Read | couldn't read "), "{caught}");
        // …and one it does not catch fails the call, with the tool's own words and the cell's line.
        let thrown = execute(
            "TsCode",
            &json!({ "code": "const before = 1;\nawait Edit({ path: 'notes/a.txt', old_string: 'nowhere', new_string: 'x' });" }),
            &ctx,
        )
        .expect_err("uncaught, so the call fails");
        assert!(thrown.contains("ToolError: old_string is not in "), "{thrown}");
        assert!(thrown.contains("at <cell 5>: await Edit("), "{thrown}");

        // Several at once are several connections, each answered.
        let together = execute(
            "TsCode",
            &json!({ "code": "(await Promise.all([1, 2, 3, 4, 5, 6].map((n) => Write({ path: `p${n}.txt`, content: String(n) })))).length;" }),
            &ctx,
        )
        .expect("six writes");
        assert_eq!(together, "6");
        assert_eq!(std::fs::read_to_string(dir.join("p6.txt")).expect("written"), "6");

        typescript.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same mode in the other language: keyword arguments or one dict, a `ToolError` to
    /// catch, and nothing else different.
    #[cfg(unix)]
    #[test]
    fn a_code_only_agent_in_python_calls_the_same_tools_as_functions() {
        let dir = scratch("only-py");
        let ctx = only_in(&dir, "only-py", CodeTools::ONLY_PYTHON, Hub::default());
        let python = crate::backends::interpreter::Interpreter::new(Language::Python, ctx.agent_dir, ctx.conv);

        let declared = declarations_with(ctx.code, &ctx.mcp.ids());
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].name, "PyCode");
        assert!(declared[0].description.contains("`Read(path=…, limit=None, offset=None)`"), "{}", declared[0].description);
        let refused = execute("TsCode", &json!({ "code": "1" }), &ctx).expect_err("the other language is not its tool");
        assert_eq!(refused, "no tool named TsCode — the tools you have are: PyCode");

        let ran = execute(
            "PyCode",
            &json!({ "code": "
wrote = Write(path='notes/a.txt', content='one\\ntwo\\n')
edited = Edit({'path': 'notes/a.txt', 'old_string': 'two', 'new_string': 'three'})
text = Read(path='notes/a.txt')
try:
    Read(path='no-such-file')
except ToolError as error:
    failed = '%s | %s' % (error.tool, str(error)[:13])
print(text)
print(failed, '|', open('notes/a.txt').read() == 'one\\nthree\\n', '|', 'a.txt:2: three' in Grep(pattern='thr+ee'))
" }),
            &ctx,
        )
        .expect("the cell runs");
        assert_eq!(ran, "one\nthree\nRead | couldn't read | True | True");

        // A call on a line of its own shows what the tool answered, as the tool said it.
        let shown = execute(
            "PyCode",
            &json!({ "code": "Write(path='notes/b.txt', content='x')\nkept = 1\nRead(path='notes/a.txt')" }),
            &ctx,
        )
        .expect("a cell of bare calls");
        let lines: Vec<&str> = shown.lines().collect();
        assert_eq!(lines.len(), 3, "{shown}");
        assert!(lines[0].starts_with("wrote ") && lines[0].ends_with("notes/b.txt (1 bytes)"), "{shown}");
        assert_eq!(&lines[1..], ["one", "three"], "{shown}");

        let raised = execute(
            "PyCode",
            &json!({ "code": "Edit(path='notes/a.txt', old_string='nowhere', new_string='x')" }),
            &ctx,
        )
        .expect_err("uncaught, so the call fails");
        assert!(raised.contains("ToolError: old_string is not in "), "{raised}");
        assert!(raised.contains("<cell 3>"), "{raised}");

        python.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One directory, whoever is asked: a relative path means to `Write` what it means to the
    /// code calling it, and a `cd` or an `export` inside `Bash` has moved the code by the time
    /// the call returns — as the code's own had moved `Bash` before it started.
    #[cfg(unix)]
    #[test]
    fn code_and_the_tools_it_calls_stand_in_one_place() {
        for (code, name, tag, moves) in [
            (
                CodeTools::ONLY_TYPESCRIPT,
                "TsCode",
                "only-place-ts",
                "process.chdir('sub');
                 process.env.FROM_CODE = 'code';
                 const wrote = await Write({ path: 'here.txt', content: 'x' });
                 const out = await Bash({ command: 'pwd; echo $FROM_CODE; cd ..; export FROM_BASH=bash' });
                 const said = out.split('\\n');
                 console.log(said[0].endsWith('/sub'), said[1], process.cwd().endsWith('/sub'), process.env.FROM_BASH, process.env.FROM_CODE);",
            ),
            (
                CodeTools::ONLY_PYTHON,
                "PyCode",
                "only-place-py",
                "os.chdir('sub')\nos.environ['FROM_CODE'] = 'code'\nwrote = Write(path='here.txt', content='x')\nsaid = Bash(command='pwd; echo $FROM_CODE; cd ..; export FROM_BASH=bash').split('\\n')\nprint(str(said[0].endswith('/sub')).lower(), said[1], str(os.getcwd().endswith('/sub')).lower(), os.environ.get('FROM_BASH'), os.environ.get('FROM_CODE'))",
            ),
        ] {
            let dir = scratch(tag);
            std::fs::create_dir_all(dir.join("sub")).expect("mkdir");
            let ctx = only_in(&dir, tag, code, Hub::default());
            let language = if name == "TsCode" { Language::TypeScript } else { Language::Python };
            let interpreter = crate::backends::interpreter::Interpreter::new(language, ctx.agent_dir, ctx.conv);

            let ran = execute(name, &json!({ "code": moves }), &ctx).expect("the cell runs");
            assert_eq!(ran, "true code false bash code", "{name}");
            assert!(dir.join("sub/here.txt").exists(), "{name}: written where the code stood");

            interpreter.stop();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The tools that are not about files work from code as they do for a model: a wake
    /// registered from a cell is a wake of this conversation's.
    #[cfg(unix)]
    #[test]
    fn a_wake_asked_for_from_code_is_this_conversations() {
        let dir = scratch("only-await");
        let ctx = only_in(&dir, "only-await", CodeTools::ONLY_TYPESCRIPT, Hub::default());
        let typescript = crate::backends::interpreter::Interpreter::new(Language::TypeScript, ctx.agent_dir, ctx.conv);

        let said = execute(
            "TsCode",
            &json!({ "code": "await Await({ after_seconds: 3600, note: 'look at the build again' });" }),
            &ctx,
        )
        .expect("registered");
        let pending = ctx.awaits.for_conversation("watcher", "conv-1");
        assert_eq!(pending.len(), 1, "{said}");
        assert_eq!(pending[0].note, "look at the build again");
        assert!(said.contains(&pending[0].id), "the cell ended with the call, so its answer is shown: {said}");

        typescript.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An MCP server attached to the agent is an object in the code: asked what it offers, called
    /// by name either way, answering with its structured result where it has one — and failing
    /// as an exception where the server says the call failed. From both languages.
    #[cfg(unix)]
    #[test]
    fn code_reaches_the_agents_mcp_servers() {
        let dir = scratch("only-mcp");
        let hub = crate::backends::mcp_hub::fake::stdio_hub(&dir);
        let ctx = only_in(&dir, "only-mcp", CodeTools::ONLY_TYPESCRIPT, hub.clone());
        let typescript = crate::backends::interpreter::Interpreter::new(Language::TypeScript, ctx.agent_dir, ctx.conv);

        let declared = declarations_with(ctx.code, &ctx.mcp.ids());
        assert!(declared[0].description.contains("`mcp.fake`"), "the server is named: {}", declared[0].description);

        let ran = execute(
            "TsCode",
            &json!({ "code": "
                const offered = await mcp.fake.tools();
                const said = await mcp.fake.echo({ text: 'hi' });
                const counted = await mcp.fake.call('count', {});
                let failed = '';
                try { await mcp.fake.fail({}); } catch (error) { failed = `${error.name}: ${error.message}`; }
                console.log(offered.map((tool) => tool.name).join(','), '|', said.split('\\n')[0], '|', counted.calls, '|', failed, '|', Object.keys(mcp).join(','));
            " }),
            &ctx,
        )
        .expect("the cell runs");
        assert_eq!(ran, "echo,count | you said hi | 2 | ToolError: no such workflow | fake");

        // The same server, from Python, in the same turn: it is the process's connection, not
        // the interpreter's — the count goes on from where TypeScript left it.
        let ctx = Ctx {
            code: CodeTools::ONLY_PYTHON,
            ..ctx
        };
        let python = crate::backends::interpreter::Interpreter::new(Language::Python, ctx.agent_dir, ctx.conv);
        let ran = execute(
            "PyCode",
            &json!({ "code": "
offered = [tool['name'] for tool in mcp.fake.tools()]
said = mcp.fake.echo(text='hi')
counted = mcp['fake'].call('count')
try:
    mcp.fake.fail()
except ToolError as error:
    failed = 'ToolError: %s' % error
print(','.join(offered), '|', said.split('\\n')[0], '|', counted['calls'], '|', failed, '|', ','.join(mcp))
" }),
            &ctx,
        )
        .expect("the cell runs");
        assert_eq!(ran, "echo,count | you said hi | 5 | ToolError: no such workflow | fake");

        typescript.stop();
        python.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nothing a cell calls outlasts the cell. A command started through `Bash` from code is
    /// stopped at the cell's own deadline, the call comes back about then rather than when the
    /// command would have ended, and the interpreter — with what it held — is still there.
    #[cfg(unix)]
    #[test]
    fn a_tool_called_from_code_ends_with_the_cells_deadline() {
        for (code, name, tag, kept, slow, look) in [
            (CodeTools::ONLY_TYPESCRIPT, "TsCode", "only-deadline-ts", "const kept = 7;", "await Bash({ command: 'sleep 30' });", "kept;"),
            (CodeTools::ONLY_PYTHON, "PyCode", "only-deadline-py", "kept = 7", "Bash(command='sleep 30')", "kept"),
        ] {
            let dir = scratch(tag);
            let ctx = only_in(&dir, tag, code, Hub::default());
            let language = if name == "TsCode" { Language::TypeScript } else { Language::Python };
            let interpreter = crate::backends::interpreter::Interpreter::new(language, ctx.agent_dir, ctx.conv);
            execute(name, &json!({ "code": kept }), &ctx).expect("a first cell");

            let began = std::time::Instant::now();
            let said = execute(name, &json!({ "code": slow, "timeout_ms": 700 }), &ctx)
                .expect_err("a timeout fails the call");
            assert!(began.elapsed() < std::time::Duration::from_secs(12), "{name}: took {:?}", began.elapsed());
            assert!(said.contains("still running after 700ms"), "{name}: {said}");
            assert!(said.contains("variables are intact"), "{name}: {said}");

            let after = execute(name, &json!({ "code": look }), &ctx).expect("the interpreter survived");
            assert_eq!(after, "7", "{name}");

            interpreter.stop();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The interpreter cannot be called from inside itself, and a name that is no tool at all is
    /// answered with the functions there are — not with the one-tool list a model is declared.
    #[cfg(unix)]
    #[test]
    fn from_code_the_code_tools_are_not_functions_and_an_unknown_name_lists_the_ones_that_are() {
        let dir = scratch("only-inner");
        let ctx = only_in(&dir, "only-inner", CodeTools::ONLY_TYPESCRIPT, Hub::default());
        let soon = std::time::Instant::now() + std::time::Duration::from_secs(5);

        let own = execute_inner("TsCode", &json!({ "code": "1" }), &ctx, soon).expect_err("refused");
        assert!(own.contains("already running in"), "{own}");
        let unknown = execute_inner("Fetch", &json!({}), &ctx, soon).expect_err("no such tool");
        assert_eq!(
            unknown,
            "no tool named Fetch — the functions there are: Read, Write, Edit, Bash, Glob, Grep, Await, Ask, Report"
        );
        assert!(
            !dir.join("conv-1.ts-kernel").exists(),
            "and no interpreter was started to say so"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A background script outlives the turn that could have answered its tool calls, so it has
    /// none — and a script that reaches for one is told that, in words, rather than that the name
    /// does not exist.
    #[cfg(unix)]
    #[test]
    fn a_background_script_of_a_code_only_agent_is_told_its_tools_are_not_there() {
        for (language, script) in [
            (Language::TypeScript, "try { await Read({ path: 'x' }); } catch (error) { console.log(error.name, '-', error.message); }"),
            (Language::Python, "try:\n    Read(path='x')\nexcept ToolError as error:\n    print('ToolError', '-', error)"),
        ] {
            let dir = scratch(&format!("only-job-{}", language.name()));
            let interpreter = crate::backends::interpreter::Interpreter::new(language, &dir, "conv-1");
            let names: Vec<&str> = inner().map(|spec| spec.name).collect();
            let command = interpreter.job_command(script, &names).expect("command");
            let out = Command::new("sh").arg("-c").arg(command).current_dir(&dir).output().expect("sh");
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(
                said.starts_with("ToolError - Read can only be called while a cell of yours is running"),
                "{}: {said}{}",
                language.name(),
                String::from_utf8_lossy(&out.stderr)
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
