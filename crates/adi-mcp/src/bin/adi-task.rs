//! `adi-task` — a tiny command-line frontend over the adi task tree.
//!
//! It shares state with the `adi-mcp` `tasks` MCP tools (the same `~/.adi/mono/mcp/tasks.json`),
//! so tasks created here show up over MCP and vice versa. All logic lives in
//! [`adi_mcp::run_tasks_cli`]; this binary is just its entry point.

fn main() -> anyhow::Result<()> {
    adi_mcp::run_tasks_cli()
}
