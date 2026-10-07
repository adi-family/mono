//! The `channel-reply` tool (`docs/channels.md` §5, "Decisions taken" #4): an **ordinary**
//! `adi-tools`-owned script, not an engine-native tool like `Ask`/`Report` — so it works
//! identically on every backend, pty/process/harness alike, rather than only the ones with a
//! native MCP tool surface.
//!
//! It reads `ADI_AGENT`/`ADI_RUN_ID` — the same pair `adi_agents::launcher::by_caller` reads — and
//! posts to this node's own small local endpoint (`POST /api/channels/reply`, built in
//! `adi-channelsd`), which is [`crate::reply::handle`] underneath: look up the
//! connection the run id maps to, then `POST /send` on the router. A mid-run post never blocks the
//! turn the way a question does, so none of the harness's suspend/resume machinery is needed —
//! which is also why this can be a plain script instead of an engine-native tool.
//!
//! TypeScript rather than `sh`: building the JSON body correctly (a reply can contain quotes,
//! newlines, anything a person types) is trivial with `JSON.stringify` and a nuisance to get right
//! in POSIX shell, and `bun run` is already how this workspace's `ts` tools execute.

use adi_tools::Tools;

use crate::error::Result;

/// The tool's stable name — also what `create_file` mints its id from, so this is also the id on
/// a fresh store (see `adi-tools`' own doc on why a tool's id is minted from its name once).
pub const TOOL_NAME: &str = "channel-reply";

/// The script sends Hive's internal domain as the Host header through its existing supervisor
/// listener. Seeded values capture the resolved flavor; its exported environment takes
/// precedence. No DNS installation or knowledge of the daemon's allocated port is required.
const SCRIPT: &str = r#"#!/usr/bin/env bun
// channel-reply — post a message back through this run's channel connection, mid-turn.
// Managed by the platform; edits are overwritten the next time this tool is (re-)seeded.
//
// Usage: channel-reply "<text>"
const text = process.argv.slice(2).join(" ");
if (!text.trim()) {
  console.error("usage: channel-reply <text>");
  process.exit(1);
}

const host = process.env.ADI_DOMAIN
  ? `channels.${process.env.ADI_DOMAIN}`
  : __CHANNELS_SERVICE_HOST__;
const hivePort = process.env.ADI_SUPERVISOR_PORT ?? __CHANNELS_HIVE_PORT__;

const res = await fetch(`http://127.0.0.1:${hivePort}/api/channels/reply`, {
  method: "POST",
  headers: { "content-type": "application/json", host },
  body: JSON.stringify({
    agent: process.env.ADI_AGENT ?? "",
    run_id: process.env.ADI_RUN_ID ?? "",
    text,
  }),
});
if (!res.ok) {
  console.error(`channel-reply failed: ${res.status} ${await res.text()}`);
  process.exit(1);
}
"#;

fn script() -> String {
    SCRIPT
        .replace(
            "__CHANNELS_SERVICE_HOST__",
            &serde_json::Value::String(crate::service::host()).to_string(),
        )
        .replace(
            "__CHANNELS_HIVE_PORT__",
            &adi_config::Flavor::current().supervisor_port.to_string(),
        )
}

/// The one line an agent sees before ever running it — `description`, in the tools listing.
const DESCRIPTION: &str = "Post a message back through this run's channel connection (Telegram/Slack) without waiting for the turn to end. Usage: channel-reply \"<text>\"";

/// Create the tool if this store has never seen it, or refresh its script if it has — the same
/// "managed, re-seeded" idempotency `adi_tools::Tools::seed_system` gives the built-in CLIs,
/// reimplemented here because that catalog is private to `adi-tools` (see the module doc on why
/// this isn't simply one more entry there).
///
/// Returns the tool's id, for whoever enables it on an agent's `bin_tools`.
///
/// # Errors
/// Whatever `adi_tools::Tools::create_file`/`write_script` returns — a write failure.
pub fn ensure(tools: &Tools) -> Result<String> {
    let script = script();
    if let Some(existing) = tools.get(TOOL_NAME)? {
        tools.write_script(&existing.id, &script)?;
        return Ok(existing.id);
    }
    let tool = tools.create_file(
        TOOL_NAME,
        Some(DESCRIPTION.to_string()),
        adi_tools::RUNTIME_TS,
        None,
        Some(script),
    )?;
    Ok(tool.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Tools {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-tool-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Tools::with_config(adi_config::Config::with_root(root))
    }

    #[test]
    fn ensure_creates_once_and_refreshes_on_every_later_call() {
        let tools = scratch("idempotent");
        let id = ensure(&tools).expect("first ensure creates it");
        assert_eq!(tools.read_script(&id).unwrap(), script());
        assert_eq!(tools.list().unwrap().len(), 1, "exactly one tool exists");

        // A second call must not create a duplicate, and must leave the script exactly as
        // managed (re-seeded), the same contract the built-in system tools carry.
        let again = ensure(&tools).expect("second ensure refreshes it");
        assert_eq!(again, id);
        assert_eq!(tools.list().unwrap().len(), 1);
        assert_eq!(tools.read_script(&id).unwrap(), script());
    }

    #[test]
    fn the_script_is_a_bun_ts_tool() {
        let tools = scratch("runtime");
        let id = ensure(&tools).unwrap();
        let tool = tools.get(&id).unwrap().unwrap();
        assert_eq!(tool.manifest.runtime, adi_tools::RUNTIME_TS);
    }
}
