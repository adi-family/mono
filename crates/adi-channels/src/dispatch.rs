//! Routing one normalized [`ChannelMessage`] to the agent its connection names
//! (`docs/channels.md` §5): allowlist enforcement, the thread->conversation map, and stamping
//! [`Marker::From`] so the agent reads the message as "somebody else, speaking through this
//! surface" rather than this machine's own voice.

use adi_agents::store::QueueMode;
use adi_agents::{Agents, LaunchOptions, Marker, launcher};
use tracing::{info, warn};

use crate::connection::{Connection, Connections, Target};
use crate::error::Result;
use crate::message::ChannelMessage;

/// Handle one inbound message: enforce the allowlist, then run or reply on the target it names.
/// Silent, by design, on everything `docs/channels.md` §5/§8 says must be silent — a paused
/// connection, a disallowed sender, a target this build doesn't dispatch to yet
/// ([`Target::Trigger`]/[`Target::AppRoute`]/[`Target::Unknown`], §6: "skipped, not refused").
///
/// # Errors
/// [`Error::NotFound`] if `message.connection` names no connection this node holds (a message for
/// a connection the router still thinks is live after this node dropped it); otherwise whatever
/// the store or the agent launch itself returns.
pub fn handle(connections: &Connections, agents: &Agents, message: &ChannelMessage) -> Result<()> {
    let connection = connections.require(&message.connection)?;
    if connection.manifest.paused {
        info!(connection = %connection.id, "connection is paused; dropping inbound message");
        return Ok(());
    }

    let Target::Agent { agent } = &connection.manifest.target else {
        info!(
            connection = %connection.id,
            target = ?connection.manifest.target,
            "connection's target isn't dispatchable yet; skipped"
        );
        return Ok(());
    };

    if !is_allowed(connections, &connection, &message.sender.id)? {
        info!(
            connection = %connection.id,
            sender = %message.sender.id,
            "sender is not on the allowlist; staying silent"
        );
        return Ok(());
    }

    let from = Marker::From {
        node: connection.manifest.provider.clone(),
        user: message.sender.name.clone(),
    };

    match connections.thread_run(&connection.id, &message.thread)? {
        Some(run_id) => match agents.reply_as(
            agent,
            &run_id,
            &message.text,
            &[],
            std::slice::from_ref(&from),
            QueueMode::Regular,
        ) {
            Ok(_sent) => Ok(()),
            // The conversation this thread pointed at is gone (deleted, or this is a fresh store) —
            // open a new one rather than failing the whole message, and re-point the thread at it.
            Err(adi_agents::Error::NotFound(_)) => {
                start_and_bind(connections, agents, &connection, agent, message, &from)
            }
            Err(e) => Err(e.into()),
        },
        None => start_and_bind(connections, agents, &connection, agent, message, &from),
    }
}

/// Whether `sender_id` may talk to `connection`'s target — claiming the first sender as the owner
/// if `owner_only` has nobody recorded yet.
///
/// **Open item, not settled by `docs/channels.md`.** §2's `linked` frame carries only
/// `{connection, provider, routing_key}` — no sender — so [`Connection::manifest.owner_sender_id`]
/// has nothing to be bound from at link time. Claiming the first sender to actually say something
/// is the best available proxy for "whoever completed the link": for Telegram (a DM) that is
/// definitionally the same person who tapped the deep link, and for a group it is at least *a*
/// member of it rather than nobody. Revisit if the router's `linked` frame ever gains a sender.
fn is_allowed(
    connections: &Connections,
    connection: &Connection,
    sender_id: &str,
) -> Result<bool> {
    if connection.manifest.owner_sender_id.is_empty()
        && connection.manifest.allowlist == crate::connection::Allowlist::OwnerOnly
    {
        connections.mark_linked(
            &connection.id,
            &connection.manifest.routing_key,
            sender_id,
        )?;
        return Ok(true);
    }
    Ok(connection
        .manifest
        .allowlist
        .allows(sender_id, &connection.manifest.owner_sender_id))
}

/// Open a fresh run for `message`'s text and bind its thread, for either a never-seen thread or
/// one whose previous conversation no longer exists.
fn start_and_bind(
    connections: &Connections,
    agents: &Agents,
    connection: &Connection,
    agent: &str,
    message: &ChannelMessage,
    from: &Marker,
) -> Result<()> {
    let launch = agents.launch(
        agent,
        &message.text,
        &LaunchOptions {
            markers: std::slice::from_ref(from),
            launched_by: Some(launcher::AUTOMATION),
            ..LaunchOptions::default()
        },
    )?;
    let Some(run_id) = conv_id_of(&launch) else {
        warn!(
            agent,
            "a pty-backed agent has no conversation id to bind a channel thread to; the reply \
             (if any) will never reach the router"
        );
        return Ok(());
    };
    connections.bind_thread(&connection.id, &message.thread, &run_id)?;
    Ok(())
}

/// The conversation id a launch opened, for the backends that keep one. `None` for a pty launch,
/// which is an interactive session with no id a later `reply_as` could target.
fn conv_id_of(launch: &adi_agents::Launch) -> Option<String> {
    match launch {
        adi_agents::Launch::Process { run_id, .. } => Some(run_id.clone()),
        adi_agents::Launch::Pty { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Allowlist;
    use crate::message::Sender;

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-dispatch-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    fn message(connection: &str, thread: &str, sender: &str, text: &str) -> ChannelMessage {
        ChannelMessage {
            v: 1,
            id: format!("evt-{thread}-{sender}"),
            provider: "telegram".into(),
            connection: connection.to_string(),
            thread: thread.to_string(),
            sender: Sender {
                id: sender.to_string(),
                name: "Igor".into(),
            },
            text: text.to_string(),
            attachments: Vec::new(),
            reply_to: None,
            raw_kind: "message".into(),
            received_at: 1,
        }
    }

    /// A connection with no matching agent: dispatch returns `Ok` and nothing is launched — the
    /// basic "named a target this build can't run" path a bad config shouldn't crash on.
    #[test]
    fn a_missing_agent_target_fails_over_launch_but_message_is_not_lost_to_a_panic() {
        let cfg = scratch("missing-agent");
        let connections = Connections::with_config(cfg.clone());
        let agents = Agents::with_config(cfg);
        let created = connections
            .create(
                "telegram",
                Target::Agent {
                    agent: "ghost".into(),
                },
            )
            .unwrap();
        connections
            .set_allowlist(&created.id, Allowlist::Open)
            .unwrap();

        let err = handle(
            &connections,
            &agents,
            &message(&created.id, "chat-1", "u1", "hi"),
        )
        .expect_err("no such agent");
        assert!(matches!(err, crate::error::Error::Agents(_)));
    }

    #[test]
    fn a_paused_connection_is_silently_skipped() {
        let cfg = scratch("paused");
        let connections = Connections::with_config(cfg.clone());
        let agents = Agents::with_config(cfg);
        let created = connections
            .create(
                "telegram",
                Target::Agent {
                    agent: "ghost".into(),
                },
            )
            .unwrap();
        connections.set_paused(&created.id, true).unwrap();

        handle(
            &connections,
            &agents,
            &message(&created.id, "chat-1", "u1", "hi"),
        )
        .expect("paused connections are skipped, not errored");
    }

    #[test]
    fn a_non_agent_target_is_skipped_not_refused() {
        let cfg = scratch("trigger-target");
        let connections = Connections::with_config(cfg.clone());
        let agents = Agents::with_config(cfg);
        let created = connections
            .create(
                "telegram",
                Target::Trigger {
                    trigger: "on-message".into(),
                },
            )
            .unwrap();

        handle(
            &connections,
            &agents,
            &message(&created.id, "chat-1", "u1", "hi"),
        )
        .expect("a target this build doesn't dispatch to is skipped, not refused");
    }

    /// The owner-claim fallback: the first sender on an unlinked `owner_only` connection becomes
    /// the owner, and a *different* sender afterwards is refused (silently — `Ok(())`, no launch).
    #[test]
    fn owner_only_claims_the_first_sender_then_locks_out_everyone_else() {
        let cfg = scratch("owner-claim");
        let connections = Connections::with_config(cfg.clone());
        let created = connections
            .create(
                "telegram",
                Target::Agent {
                    agent: "ghost".into(),
                },
            )
            .unwrap();
        assert!(is_allowed(&connections, &created, "first").unwrap());
        let claimed = connections.require(&created.id).unwrap();
        assert_eq!(claimed.manifest.owner_sender_id, "first");

        assert!(is_allowed(&connections, &claimed, "first").unwrap());
        assert!(!is_allowed(&connections, &claimed, "someone-else").unwrap());
    }
}
