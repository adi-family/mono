//! adi-mesh (CLI) — a thin argv adapter over the [`adi_mesh`] library.
//!
//! `run` starts the in-process [`Daemon`] and waits for a signal — the same runtime the
//! control-panel [`adi-app`](../adi-app) starts in-process behind its "Start mesh" button,
//! so the daemon never has to be a separately-launched process. The other subcommands edit
//! the config or print this machine's identity/ticket.

use adi_mesh::config::{Forward, MeshConfig};
use adi_mesh::{Daemon, identity, ticket};
use clap::{Parser, Subcommand};
use tracing::info;

#[derive(Debug, Parser)]
#[command(
    name = "adi-mesh",
    about = "Connect adi machines peer-to-peer over iroh: expose allow-listed local ports to authorized peers, and forward a local port to a peer's port.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the mesh daemon: serve allowed ports and open every configured forward (default).
    Run,
    /// Print this machine's `EndpointId` — the minimal token a peer can dial (via discovery).
    Id,
    /// Print a shareable ticket (id + relay + direct addresses) — briefly goes online.
    Ticket,
    /// Expose a local TCP port to peers.
    Allow {
        /// The local port to allow (`127.0.0.1:<port>`).
        port: u16,
    },
    /// Stop exposing a local TCP port.
    Deny {
        /// The local port to remove from the allow-list.
        port: u16,
    },
    /// Restrict allowed ports to a specific peer (repeatable). With none set, any peer may use them.
    AllowPeer {
        /// The peer's `EndpointId` or ticket to authorize (stored as its id).
        peer: String,
    },
    /// Forward a local port to a peer's port (bind `127.0.0.1:<listen>` → peer's `<port>`).
    Forward {
        /// Local TCP port to bind on this machine.
        listen: u16,
        /// The peer's ticket (from `adi-mesh ticket`) or bare `EndpointId` (from `adi-mesh id`).
        peer: String,
        /// The port to reach on the peer (must be on the peer's allow-list).
        port: u16,
        /// An optional label for logs and `list`.
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove the forward bound to a local port.
    Unforward {
        /// The local `listen` port whose forward to remove.
        listen: u16,
    },
    /// Show this machine's id, allowed ports, authorized peers, and forwards.
    List,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command.unwrap_or(Command::Run) {
        Command::Run => run().await,
        Command::Id => {
            println!("{}", identity::endpoint_id()?);
            Ok(())
        }
        Command::Ticket => {
            println!("{}", adi_mesh::current_ticket().await?);
            Ok(())
        }
        Command::Allow { port } => edit(move |cfg| {
            let added = cfg.allow_port(port);
            let msg = if added {
                format!("allowed port {port}")
            } else {
                format!("port {port} was already allowed")
            };
            (added, msg)
        }),
        Command::Deny { port } => edit(|cfg| {
            let removed = cfg.deny_port(port);
            let msg = if removed {
                format!("denied port {port}")
            } else {
                format!("port {port} was not allowed")
            };
            (removed, msg)
        }),
        Command::AllowPeer { peer } => {
            // Store the canonical id, whether an id or a full ticket was given.
            let id = ticket::target_id(&peer)?.to_string();
            edit(move |cfg| {
                let added = cfg.allow_peer(id.clone());
                let msg = if added {
                    format!("authorized peer {id}")
                } else {
                    format!("peer {id} was already authorized")
                };
                (added, msg)
            })
        }
        Command::Forward {
            listen,
            peer,
            port,
            name,
        } => {
            // Validate the target now (ticket or id) so a bad token fails fast.
            let id = ticket::target_id(&peer)?;
            let name = name.unwrap_or_else(|| default_forward_name(&id.to_string(), port));
            edit(move |cfg| {
                let replaced = cfg.add_forward(Forward {
                    name: name.clone(),
                    listen,
                    peer: peer.clone(),
                    port,
                });
                let verb = if replaced { "replaced" } else { "added" };
                (
                    true,
                    format!("{verb} forward {name}: 127.0.0.1:{listen} -> peer:{port}"),
                )
            })
        }
        Command::Unforward { listen } => edit(|cfg| {
            let removed = cfg.remove_forward(listen);
            let msg = if removed {
                format!("removed the forward on local port {listen}")
            } else {
                format!("no forward was bound to local port {listen}")
            };
            (removed, msg)
        }),
        Command::List => list(),
    }
}

/// Start the in-process daemon and run until a shutdown signal.
async fn run() -> anyhow::Result<()> {
    init_tracing();
    let daemon = Daemon::start().await?;
    shutdown_signal().await;
    info!("shutdown signal received; stopping");
    daemon.stop().await;
    Ok(())
}

/// Load the config, apply `edit` (which returns `(changed, message)`), save if it changed,
/// and print the message.
fn edit(edit: impl FnOnce(&mut MeshConfig) -> (bool, String)) -> anyhow::Result<()> {
    let mut cfg = MeshConfig::load()?;
    let (changed, message) = edit(&mut cfg);
    if changed {
        cfg.save()?;
    }
    println!("{message}");
    Ok(())
}

/// Print the current identity and config in a human-readable form.
fn list() -> anyhow::Result<()> {
    let cfg = MeshConfig::load()?;
    println!("id: {}", identity::endpoint_id()?);

    if cfg.host.allow.is_empty() {
        println!("allowed ports: (none)");
    } else {
        let ports = cfg
            .host
            .allow
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!("allowed ports: {ports}");
    }

    if cfg.host.authorized_peers.is_empty() {
        println!("authorized peers: (any)");
    } else {
        for peer in &cfg.host.authorized_peers {
            println!("authorized peer: {peer}");
        }
    }

    if cfg.forwards.is_empty() {
        println!("forwards: (none)");
    } else {
        for f in &cfg.forwards {
            println!(
                "forward {}: 127.0.0.1:{} -> {}:{}",
                f.name, f.listen, f.peer, f.port
            );
        }
    }
    Ok(())
}

/// A readable default forward label: the peer id's short prefix and the remote port.
fn default_forward_name(peer_id: &str, port: u16) -> String {
    let prefix: String = peer_id.chars().take(8).collect();
    format!("{prefix}:{port}")
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = term.recv() => {},
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGTERM handler; using ctrl-c only");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
