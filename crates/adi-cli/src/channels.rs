//! The `channels` command group (`docs/channels.md` §7): connect a service to an agent, see what
//! this node holds, change where a connection goes, who may talk to it, and tear it down.
//!
//! Every verb here is a thin wrapper over this node's own `/api/channels/*`
//! (`adi_channels::node_api::NodeApi`) — never the router directly, and never this node's local
//! store files either. The panel calls the exact same endpoints from the browser; this group is
//! the shell's way in, and the two are kept honest by sharing one contract rather than each
//! reimplementing it. Hive runs the `adi-channelsd` service and publishes its internal domain;
//! `connect` waits on that service's WebSocket link state, independently of the control panel.

use adi_channels::connection::{Allowlist, Target};
use adi_channels::node_api::{ConnectionView, NodeApi};
use clap::Subcommand;

use crate::format::print_json;

#[derive(Debug, Subcommand)]
pub(crate) enum ChannelsCommand {
    /// Connect a service to an agent: register this node for it, print the install/link URL, and
    /// wait for the operator to actually complete the link.
    Connect {
        /// `telegram` or `slack`.
        provider: String,
        /// The agent this connection's messages run against.
        #[arg(long)]
        agent: String,
        /// Draw the install/link URL as a terminal QR code too (on by default at a terminal).
        #[arg(long)]
        qr: bool,
        /// Never draw the QR code, however the output is going.
        #[arg(long, conflicts_with = "qr")]
        no_qr: bool,
        /// Give up waiting after this many seconds and leave the connection unlinked — it can
        /// still be completed later; this just stops blocking the shell.
        #[arg(long, default_value_t = 600)]
        timeout_secs: u64,
        #[arg(long)]
        json: bool,
    },
    /// List every connection this node holds.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Change a connection's target agent, or pause/resume delivery to it.
    Route {
        id: String,
        /// Point this connection at a different agent.
        #[arg(long)]
        agent: Option<String>,
        /// Stop delivering to the target without tearing the connection down.
        #[arg(long, conflicts_with = "resume")]
        pause: bool,
        /// Resume delivery.
        #[arg(long, conflicts_with = "pause")]
        resume: bool,
        #[arg(long)]
        json: bool,
    },
    /// Change who may talk to a connection's target.
    Allow {
        id: String,
        /// Add one sender id to the allowlist — narrows an `open`/`owner_only` connection to just
        /// this sender plus whoever is already explicitly listed.
        #[arg(long, conflicts_with = "mode")]
        add: Option<String>,
        /// Replace the allowlist outright: `open` (anyone in the chat/workspace) or `owner_only`
        /// (only whoever completed the link — the default).
        #[arg(long, value_parser = ["open", "owner_only"], conflicts_with = "add")]
        mode: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Tell the router to drop a connection, then drop it here too.
    Disconnect {
        id: String,
        #[arg(long)]
        json: bool,
    },
}

/// Dispatch a `channels` subcommand against this node's own `/api/channels/*`.
///
/// # Errors
/// Whatever the API call returns, including a connection error when the Hive-managed channels
/// service is unavailable.
pub(crate) fn run_channels(command: ChannelsCommand) -> Result<(), String> {
    let config = adi_config::Config::open();
    let api = NodeApi::local(&config)?;
    match command {
        ChannelsCommand::Connect {
            provider,
            agent,
            qr,
            no_qr,
            timeout_secs,
            json,
        } => connect(&api, &provider, &agent, qr, no_qr, timeout_secs, json),
        ChannelsCommand::List { json } => list(&api, json),
        ChannelsCommand::Route {
            id,
            agent,
            pause,
            resume,
            json,
        } => route(&api, &id, agent.as_deref(), pause, resume, json),
        ChannelsCommand::Allow {
            id,
            add,
            mode,
            json,
        } => allow(&api, &id, add.as_deref(), mode.as_deref(), json),
        ChannelsCommand::Disconnect { id, json } => disconnect(&api, &id, json),
    }
}

// -- connect -----------------------------------------------------------------------------------

/// Register this node for `provider`, show the install/link URL, and block until the operator
/// has actually completed the link (or `timeout_secs` runs out).
///
/// The QR default follows `mesh invite`'s own rule exactly (see `crate::mesh::invite`): on by
/// default at a terminal, off everywhere else, so every script that captures this command's
/// output keeps working byte for byte. `--json` never carries one.
fn connect(
    api: &NodeApi,
    provider: &str,
    agent: &str,
    qr: bool,
    no_qr: bool,
    timeout_secs: u64,
    json: bool,
) -> Result<(), String> {
    use std::io::IsTerminal as _;

    let connected = api.connect(
        provider,
        &Target::Agent {
            agent: agent.to_string(),
        },
    )?;
    let id = connected.connection.id.clone();
    let install_url = connected.install_url.clone();
    let install_url_group = connected.install_url_group.clone();

    if !json {
        if install_url.is_empty() {
            println!(
                "Registered connection {id} for {provider}, but this build has no install/link \
                 URL for it yet — complete the link however that service's install flow works."
            );
        } else {
            println!("Connection {id} — complete the link at:");
            println!("  {install_url}");
            if !install_url_group.is_empty() {
                println!("Or add it to a group:");
                println!("  {install_url_group}");
            }
            if qr || (!no_qr && std::io::stdout().is_terminal()) {
                match crate::qr::terminal(&install_url) {
                    Ok(code) => {
                        println!();
                        print!("{}", code.text);
                        println!(
                            "({} columns by {} rows — resize this window if the code came out \
                             broken up)",
                            code.columns, code.rows
                        );
                    }
                    Err(e) => eprintln!("note: {e}"),
                }
            }
        }
        println!("Waiting for the link to complete (Ctrl-C to stop waiting — the connection stays)...");
    }

    let linked = wait_for_link(api, &id, std::time::Duration::from_secs(timeout_secs));

    if json {
        print_json(&serde_json::json!({
            "connection": id,
            "install_url": install_url,
            "install_url_group": install_url_group,
            "linked": linked,
        }));
        return Ok(());
    }
    if linked {
        println!("Linked.");
    } else {
        println!(
            "Still not linked after {timeout_secs}s — the connection is kept, run \
             `adi-mono channels list` to check on it later."
        );
    }
    Ok(())
}

/// Poll `GET /api/channels/<id>` until `linked` or `deadline` passes. The same shape
/// `crate::agents::await_run` polls a run's own state with — a plain sleep loop, since this is a
/// person waiting at a shell and not a server holding a connection open.
fn wait_for_link(api: &NodeApi, id: &str, timeout: std::time::Duration) -> bool {
    const LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match api.get(id) {
            Ok(connection) if connection.linked => return true,
            _ => {}
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(LOOK_EVERY);
    }
}

// -- list --------------------------------------------------------------------------------------

fn list(api: &NodeApi, json: bool) -> Result<(), String> {
    let connections = api.list()?;
    if json {
        print_json(&connections_json(&connections));
        return Ok(());
    }
    if connections.is_empty() {
        println!("No channel connections on this node yet. Run `adi-mono channels connect <svc> --agent <a>`.");
        return Ok(());
    }
    for c in &connections {
        print_connection(c);
    }
    Ok(())
}

fn print_connection(c: &ConnectionView) {
    let state = match (c.linked, c.paused) {
        (false, _) => "pending link",
        (true, true) => "paused",
        (true, false) => "live",
    };
    println!("{} — {} [{state}]", c.id, c.provider);
    println!("  target: {}", target_label(&c.target));
    println!("  who may talk: {}", allowlist_label(&c.allowlist));
    if c.linked {
        println!("  routing key: {}", c.routing_key);
    }
}

fn connections_json(connections: &[ConnectionView]) -> Vec<serde_json::Value> {
    connections
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id,
                "provider": c.provider,
                "routing_key": c.routing_key,
                "target": target_label(&c.target),
                "allowlist": allowlist_label(&c.allowlist),
                "paused": c.paused,
                "linked": c.linked,
                "created_at": c.created_at,
                "updated_at": c.updated_at,
            })
        })
        .collect()
}

fn target_label(target: &Target) -> String {
    match target {
        Target::Agent { agent } => format!("agent:{agent}"),
        Target::Trigger { trigger } => format!("trigger:{trigger}"),
        Target::AppRoute { app, route } => format!("app:{app}/{route}"),
        Target::Unknown => "unknown".to_string(),
    }
}

fn allowlist_label(allowlist: &Allowlist) -> String {
    match allowlist {
        Allowlist::OwnerOnly => "owner only".to_string(),
        Allowlist::Open => "open".to_string(),
        Allowlist::List { sender_ids } => format!("list: {}", sender_ids.join(", ")),
    }
}

// -- route -------------------------------------------------------------------------------------

/// Change a connection's target agent, or pause/resume it, or both in one call.
fn route(
    api: &NodeApi,
    id: &str,
    agent: Option<&str>,
    pause: bool,
    resume: bool,
    json: bool,
) -> Result<(), String> {
    if agent.is_none() && !pause && !resume {
        return Err("nothing to change: give --agent, --pause, or --resume".to_string());
    }
    let mut connections = Vec::new();
    if let Some(agent) = agent {
        connections = api.route(
            id,
            &Target::Agent {
                agent: agent.to_string(),
            },
        )?;
    }
    if pause || resume {
        connections = api.pause(id, pause)?;
    }
    report_one(id, &connections, json, "updated");
    Ok(())
}

// -- allow -------------------------------------------------------------------------------------

fn allow(
    api: &NodeApi,
    id: &str,
    add: Option<&str>,
    mode: Option<&str>,
    json: bool,
) -> Result<(), String> {
    let allowlist = match (add, mode) {
        (Some(sender), None) => {
            let mut sender_ids = match api.get(id)?.allowlist {
                Allowlist::List { sender_ids } => sender_ids,
                Allowlist::OwnerOnly | Allowlist::Open => Vec::new(),
            };
            if !sender_ids.iter().any(|s| s == sender) {
                sender_ids.push(sender.to_string());
            }
            Allowlist::List { sender_ids }
        }
        (None, Some("open")) => Allowlist::Open,
        (None, Some("owner_only")) => Allowlist::OwnerOnly,
        (None, Some(other)) => return Err(format!("unknown --mode {other:?}")),
        (None, None) => return Err("give --add <sender> or --mode open|owner_only".to_string()),
        (Some(_), Some(_)) => unreachable!("clap already refuses --add with --mode"),
    };
    let connections = api.allow(id, &allowlist)?;
    report_one(id, &connections, json, "updated");
    Ok(())
}

// -- disconnect ----------------------------------------------------------------------------------

fn disconnect(api: &NodeApi, id: &str, json: bool) -> Result<(), String> {
    let connections = api.disconnect(id)?;
    if json {
        print_json(&serde_json::json!({ "id": id, "disconnected": true }));
        return Ok(());
    }
    println!("Disconnected {id}.");
    let _ = connections; // the fresh list, not needed for this report
    Ok(())
}

// -- shared --------------------------------------------------------------------------------------

/// Print the one connection `id` names out of a fresh list a mutation returned, or just say what
/// happened in JSON mode.
fn report_one(id: &str, connections: &[ConnectionView], json: bool, verb: &str) {
    if json {
        print_json(&connections_json(connections));
        return;
    }
    match connections.iter().find(|c| c.id == id) {
        Some(c) => {
            println!("{verb} {id}:");
            print_connection(c);
        }
        None => println!("{verb} {id}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory as _, Parser};

    #[derive(Debug, Parser)]
    #[command(name = "channels")]
    struct Harness {
        #[command(subcommand)]
        command: ChannelsCommand,
    }

    fn command_line(words: &[&str]) -> Vec<String> {
        std::iter::once("channels")
            .chain(words.iter().copied())
            .map(ToString::to_string)
            .collect()
    }

    fn parse(words: &[&str]) -> ChannelsCommand {
        Harness::try_parse_from(command_line(words))
            .unwrap_or_else(|e| panic!("{words:?} should parse: {e}"))
            .command
    }

    fn rejects(words: &[&str]) {
        assert!(
            Harness::try_parse_from(command_line(words)).is_err(),
            "{words:?} should be rejected"
        );
    }

    #[test]
    fn the_group_is_a_well_formed_clap_tree() {
        Harness::command().debug_assert();
    }

    #[test]
    fn connect_takes_the_provider_positionally_and_the_agent_as_a_flag() {
        match parse(&["connect", "telegram", "--agent", "adi-agent"]) {
            ChannelsCommand::Connect {
                provider,
                agent,
                qr,
                no_qr,
                timeout_secs,
                json,
            } => {
                assert_eq!(provider, "telegram");
                assert_eq!(agent, "adi-agent");
                assert!(!qr && !no_qr && !json);
                assert_eq!(timeout_secs, 600);
            }
            other => panic!("expected connect, got {other:?}"),
        }
        rejects(&["connect", "telegram"]); // --agent is required
        rejects(&["connect", "telegram", "--agent", "a", "--qr", "--no-qr"]);
    }

    #[test]
    fn route_rejects_pause_and_resume_together_but_takes_either_alone() {
        match parse(&["route", "c1", "--agent", "b"]) {
            ChannelsCommand::Route {
                id, agent, pause, resume, ..
            } => {
                assert_eq!(id, "c1");
                assert_eq!(agent.as_deref(), Some("b"));
                assert!(!pause && !resume);
            }
            other => panic!("expected route, got {other:?}"),
        }
        assert!(matches!(
            parse(&["route", "c1", "--pause"]),
            ChannelsCommand::Route { pause: true, .. }
        ));
        assert!(matches!(
            parse(&["route", "c1", "--resume"]),
            ChannelsCommand::Route { resume: true, .. }
        ));
        rejects(&["route", "c1", "--pause", "--resume"]);
    }

    #[test]
    fn allow_rejects_add_and_mode_together_but_takes_either_alone() {
        match parse(&["allow", "c1", "--add", "u1"]) {
            ChannelsCommand::Allow { id, add, mode, .. } => {
                assert_eq!(id, "c1");
                assert_eq!(add.as_deref(), Some("u1"));
                assert!(mode.is_none());
            }
            other => panic!("expected allow, got {other:?}"),
        }
        match parse(&["allow", "c1", "--mode", "open"]) {
            ChannelsCommand::Allow { mode, .. } => assert_eq!(mode.as_deref(), Some("open")),
            other => panic!("expected allow, got {other:?}"),
        }
        rejects(&["allow", "c1", "--mode", "wide-open"]); // not one of the two accepted values
        rejects(&["allow", "c1", "--add", "u1", "--mode", "open"]);
    }

    #[test]
    fn disconnect_takes_the_id_positionally() {
        match parse(&["disconnect", "c1"]) {
            ChannelsCommand::Disconnect { id, json } => {
                assert_eq!(id, "c1");
                assert!(!json);
            }
            other => panic!("expected disconnect, got {other:?}"),
        }
        rejects(&["disconnect"]);
    }

    #[test]
    fn target_and_allowlist_labels_read_as_one_short_line() {
        assert_eq!(
            target_label(&Target::Agent { agent: "a".into() }),
            "agent:a"
        );
        assert_eq!(allowlist_label(&Allowlist::OwnerOnly), "owner only");
        assert_eq!(allowlist_label(&Allowlist::Open), "open");
        assert_eq!(
            allowlist_label(&Allowlist::List { sender_ids: vec!["a".into(), "b".into()] }),
            "list: a, b"
        );
    }
}
