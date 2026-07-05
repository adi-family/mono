//! adi-mono — the adi platform CLI. A thin argv adapter over `adi-core`'s command
//! surface: every subcommand maps 1:1 to a method call (`adi-mono dns enable` →
//! `Adi::new().dns().enable()`), so the GUI can trigger platform actions by running
//! this binary. Named `adi-mono` today; slated to become `adi`.

use adi_core::{Adi, Report, Service, ServiceReport};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "adi-mono", about = "Control the adi platform.", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
}

#[derive(Debug, Subcommand)]
enum DnsCommand {
    /// Enable the DNS resolver (installs the route + landing page on first enable).
    Enable,
    /// Disable the DNS resolver (leaves the route + landing page in place).
    Disable,
    /// Show live DNS status.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Install the `.adi` route + landing page (one admin prompt).
    InstallRoute,
    /// Remove the `.adi` route + landing page (one admin prompt).
    RemoveRoute,
}

fn main() {
    let cli = Cli::parse();
    let adi = Adi::new();
    match cli.command {
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
    }
}

fn print_report(report: &Report, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string())
        );
        return;
    }
    for svc in &report.services {
        print_human(svc);
    }
}

fn print_service(svc: &ServiceReport, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(svc).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        print_human(svc);
    }
}

fn print_human(svc: &ServiceReport) {
    let state = match (svc.enabled, svc.running) {
        (_, true) => "running",
        (true, false) => "enabled",
        (false, false) => "stopped",
    };
    println!("{} — {} [{state}]", svc.name, svc.detail);
    for action in &svc.actions {
        println!(
            "  {}: {}  (adi-mono {})",
            action.id,
            action.title,
            action.args.join(" ")
        );
    }
}
