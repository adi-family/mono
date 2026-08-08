pub(crate) mod adi_events;
pub(crate) mod claude_stream;
pub(crate) mod detached;
pub(crate) mod harness;
// A `Bash` command that outlives the turn that started it, and the wake that reports it.
pub(crate) mod jobs;
// The MCP door into [`harness::tools`], for the Claude engines whose loop — and whose tool set — is
// their own. A sibling of `harness` rather than a child: it serves that table, it is not part of it.
pub(crate) mod mcp;
pub(crate) mod process;
pub(crate) mod pty;
// The shell a conversation keeps between commands. Here rather than under one engine because two
// engines reach it: the adi loop's own `Bash` runs through it, and the Claude CLI's `Bash` is bent
// through it by the hook the runner installs.
pub(crate) mod shell;

pub(crate) fn push_option(argv: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        argv.extend([flag.into(), value.into()]);
    }
}
