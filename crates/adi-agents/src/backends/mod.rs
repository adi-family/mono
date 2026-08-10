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

/// Push one run's tool surface onto a Claude CLI command line — the same two flags, in the same
/// order, for every engine that speaks this CLI (see [`mcp::ToolScope`] for what each one does).
///
/// `--tools` is pushed *always*, empty included: an empty value is the CLI's spelling of "no
/// built-ins at all", and it is exactly the case [`push_option`] would drop — leaving the flag off
/// hands the run every built-in the engine ships, which is the opposite of what was asked for.
pub(crate) fn push_tool_scope(argv: &mut Vec<String>, scope: &mcp::ToolScope) {
    argv.extend(["--tools".to_string(), scope.builtins.clone()]);
    push_option(argv, "--allowed-tools", Some(&scope.allowed));
}
