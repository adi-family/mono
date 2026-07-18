//! adi-mono — the adi platform CLI: a thin argv adapter over `adi-core`'s command
//! surface where every subcommand maps 1:1 to a method call, so the GUI can trigger
//! platform actions by running this binary.

mod agents;
mod dns;
mod format;
mod projects;
mod tasks;
mod triggers;
mod update;

use adi_core::{Adi, Service};
use clap::{Parser, Subcommand};

use crate::agents::{AgentsCommand, run_agents};
use crate::dns::DnsCommand;
use crate::format::{print_report, print_service};
use crate::projects::{ProjectsCommand, run_projects};
use crate::tasks::{TasksCommand, run_tasks};
use crate::triggers::{TriggersCommand, run_triggers};
use crate::update::{UpdateCommand, run_update};

#[derive(Debug, Parser)]
#[command(name = "adi-mono", about = "Control the adi platform.", version)]
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
    /// Auto-update commands: one update swaps the whole app bundle (every binary).
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
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
        Command::Update { command } => run_update(adi, command),
    }
}
