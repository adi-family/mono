//! adi-mono — the adi platform CLI: a thin argv adapter over `adi-core`'s command
//! surface where every subcommand maps 1:1 to a method call, so the GUI can trigger
//! platform actions by running this binary.

mod agents;
mod db;
mod dns;
mod format;
mod mesh;
mod projects;
mod secrets;
mod events;
mod tasks;
mod tools;
mod triggers;
mod update;

use adi_core::{Adi, Service, VERSION};
use clap::{Parser, Subcommand};

use crate::agents::{AgentsCommand, run_agents};
use crate::db::{DbCommand, run_db};
use crate::dns::DnsCommand;
use crate::format::{print_report, print_service};
use crate::mesh::{MeshCommand, run_mesh};
use crate::projects::{ProjectsCommand, run_projects};
use crate::secrets::{SecretsCommand, run_secrets};
use crate::events::{EventsCommand, run_events};
use crate::tasks::{TasksCommand, run_tasks};
use crate::tools::{ToolsCommand, run_tools};
use crate::triggers::{TriggersCommand, run_triggers};
use crate::update::{UpdateCommand, run_update};

#[derive(Debug, Parser)]
// `version = VERSION`, not clap's default `version`: the default is CARGO_PKG_VERSION, which
// stays at the workspace floor and would report an older number than the bundle it ships in.
#[command(name = "adi-mono", about = "Control the adi platform.", version = VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Bring every service up if not already running (the launch-time bootstrap; never
    /// restarts a running service). Safe to run on every app launch.
    Up,
    /// Enable every adi service.
    Enable,
    /// Disable every adi service.
    Disable,
    /// Show live status across all services.
    Status {
        /// Emit machine-readable JSON (what the GUI polls).
        #[arg(long)]
        json: bool,
    },
    /// DNS resolver commands.
    Dns {
        #[command(subcommand)]
        command: DnsCommand,
    },
    /// Project registry commands (metadata under ~/.adi/mono/projects).
    Projects {
        #[command(subcommand)]
        command: ProjectsCommand,
    },
    /// Task tree commands.
    Tasks {
        #[command(subcommand)]
        command: TasksCommand,
    },
    /// Tool commands: user CLIs (sh/ts) created in-store or linked by path, run by agents.
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
    /// Secret commands: encrypted global / per-project key-values, injected into runs.
    Secrets {
        #[command(subcommand)]
        command: SecretsCommand,
    },
    /// Database commands: run SQL against the shared SQLite store (global or per-project).
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Agent definition commands.
    Agents {
        #[command(subcommand)]
        command: AgentsCommand,
    },
    /// Trigger commands (background code blocks fired by webhooks & co.).
    Triggers {
        #[command(subcommand)]
        command: TriggersCommand,
    },
    /// Fleet commands: pair remote adi machines over the mesh, and say what they may reach.
    Mesh {
        #[command(subcommand)]
        command: MeshCommand,
    },
    /// Event bus commands: publish platform events and peek at the spool.
    Events {
        #[command(subcommand)]
        command: EventsCommand,
    },
    /// Auto-update commands: one update swaps the whole app bundle (every binary).
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
    /// Internal: run one turn of a `harness:adi` conversation — read its transcript, call the
    /// provider, and print the answer. Spawned by the app for each `adi` turn; not for direct use.
    #[command(hide = true)]
    HarnessTurn {
        /// The agent whose conversation this turn belongs to.
        #[arg(long)]
        agent: String,
        /// The conversation (run) id to answer into.
        #[arg(long)]
        conv: String,
    },
}

fn main() {
    let cli = Cli::parse();
    let adi = Adi::new();
    match cli.command {
        Command::Up => adi.ensure_enabled(),
        Command::Enable => adi.enable(),
        Command::Disable => adi.disable(),
        Command::Status { json } => print_report(&adi.report(), json),
        Command::Dns { command } => match command {
            DnsCommand::Enable => adi.dns().enable(),
            DnsCommand::Disable => adi.dns().disable(),
            DnsCommand::Status { json } => print_service(&adi.dns().report(), json),
            DnsCommand::InstallRoute => adi.dns().install_route(),
            DnsCommand::RemoveRoute => adi.dns().remove_route(),
        },
        Command::Projects { command } => {
            if let Err(e) = run_projects(adi, command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Command::Tasks { command } => {
            if let Err(e) = run_tasks(adi, command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Command::Tools { command } => {
            if let Err(e) = run_tools(adi, command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Command::Secrets { command } => {
            if let Err(e) = run_secrets(adi, command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Command::Db { command } => {
            if let Err(e) = run_db(adi, command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Command::Agents { command } => {
            if let Err(e) = run_agents(adi, command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Command::Triggers { command } => {
            if let Err(e) = run_triggers(adi, command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        // The one group that does not take the `adi` facade: its state is the mesh module's own.
        Command::Mesh { command } => {
            if let Err(e) = run_mesh(command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Command::Events { command } => {
            if let Err(e) = run_events(adi, command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Command::Update { command } => run_update(adi, command),
        // The answer (or a readable error) is this process's stdout, which the spawning conversation
        // captures as the turn's output and folds into the transcript. Flush explicitly: a
        // `process::exit` skips the normal stdout flush.
        Command::HarnessTurn { agent, conv } => {
            use std::io::Write as _;
            // The loop writes its own events — commentary, each tool call, the answer — to stdout
            // as it goes, so nothing is printed here on success: doing so would say the answer
            // twice. A failure has written no answer event, and its one plain line is read back as
            // the turn's text.
            let mut out = std::io::stdout();
            let code = match adi.agents().run_adi_turn(&agent, &conv, &mut out) {
                Ok(_) => 0,
                Err(e) => {
                    let _ = writeln!(out, "⚠ adi loop error: {e}");
                    1
                }
            };
            let _ = out.flush();
            std::process::exit(code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn the_command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_mesh_group_is_reachable_from_the_top_level() {
        // The group's own argv surface is tested in `mesh.rs`; what this pins is the wiring —
        // that `adi-mono mesh …` reaches it at all, which a module compiled but never registered
        // would pass every one of those tests without.
        let cli = Cli::try_parse_from(["adi-mono", "mesh", "fleet", "--json"]).expect("parses");
        assert!(matches!(
            cli.command,
            Command::Mesh {
                command: MeshCommand::Fleet { json: true }
            }
        ));
        assert!(Cli::try_parse_from(["adi-mono", "mesh"]).is_err());
    }
}
