pub(crate) mod adi_events;
pub(crate) mod claude_stream;
pub(crate) mod detached;
pub(crate) mod harness;
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
