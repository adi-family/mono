//! adi-mono — the adi platform CLI: a thin argv adapter over `adi-core`'s command
//! surface where every subcommand maps 1:1 to a method call, so the GUI can trigger
//! platform actions by running this binary.

use adi_core::{Adi, Project, Report, Service, ServiceReport};
use clap::{Parser, Subcommand};

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
}

#[derive(Debug, Subcommand)]
enum DnsCommand {
    /// Enable the DNS resolver (installs the route + front-door proxy on first enable).
    Enable,
    /// Disable the DNS resolver (leaves the route + front-door proxy in place).
    Disable,
    /// Show live DNS status.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Install the `.adi` route + front-door proxy (one admin prompt).
    InstallRoute,
    /// Remove the `.adi` route + front-door proxy (one admin prompt).
    RemoveRoute,
}

#[derive(Debug, Subcommand)]
enum ProjectsCommand {
    /// List registered projects (active only unless `--all`).
    List {
        /// Include archived projects.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Register a new project (writes projects/<id>/config.toml).
    Add {
        /// The project id — its directory name (letters, digits, '.', '-', '_').
        id: String,
        /// A display name; defaults to the id.
        #[arg(long)]
        name: Option<String>,
        /// An optional one-line description.
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show one project's manifest.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Archive a project (soft delete; reversible with `unarchive`).
    Archive { id: String },
    /// Restore an archived project.
    Unarchive { id: String },
    /// Permanently delete a project's directory.
    Rm { id: String },
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
    }
}

/// Dispatch a `projects` subcommand over the adi-core facade, surfacing any store error.
fn run_projects(adi: Adi, command: ProjectsCommand) -> Result<(), adi_core::ProjectsError> {
    let store = adi.projects();
    match command {
        ProjectsCommand::List { all, json } => {
            let mut projects = store.list()?;
            if !all {
                projects.retain(|p| !p.is_archived());
            }
            if json {
                print_json(&projects);
            } else if projects.is_empty() {
                println!("No projects registered.");
            } else {
                for project in &projects {
                    print_project(project);
                }
            }
        }
        ProjectsCommand::Add {
            id,
            name,
            description,
            json,
        } => {
            let project = store.create(&id, name, description)?;
            if json {
                print_json(&project);
            } else {
                println!("Registered project {}.", project.id);
                print_project(&project);
            }
        }
        ProjectsCommand::Show { id, json } => {
            let project = store
                .get(&id)?
                .ok_or_else(|| adi_core::ProjectsError::NotFound(id.clone()))?;
            if json {
                print_json(&project);
            } else {
                print_project(&project);
            }
        }
        ProjectsCommand::Archive { id } => {
            let project = store.archive(&id)?;
            println!("Archived {}.", project.id);
        }
        ProjectsCommand::Unarchive { id } => {
            let project = store.unarchive(&id)?;
            println!("Restored {}.", project.id);
        }
        ProjectsCommand::Rm { id } => {
            if store.remove(&id)? {
                println!("Deleted project {id}.");
            } else {
                println!("No such project: {id}.");
            }
        }
    }
    Ok(())
}

/// Print a project as a human line plus its description, mirroring `print_human` for services.
fn print_project(project: &Project) {
    let state = if project.is_archived() {
        "archived"
    } else {
        "active"
    };
    println!("{} — {} [{state}]", project.id, project.display_name());
    if let Some(description) = &project.manifest.description {
        println!("  {description}");
    }
}

/// Serialize any value to pretty JSON, degrading to `{}` on the (unreachable) encode failure.
fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
    );
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
