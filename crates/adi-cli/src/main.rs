//! adi-mono — the adi platform CLI: a thin argv adapter over `adi-core`'s command
//! surface where every subcommand maps 1:1 to a method call, so the GUI can trigger
//! platform actions by running this binary.

mod agents;
mod db;
mod dns;
mod format;
mod goals;
mod indexer;
mod facts;
mod knowledge;
mod mesh;
mod projects;
mod reader;
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
use crate::goals::{GoalsCommand, run_goals};
use crate::indexer::{IndexerCommand, run_indexer};
use crate::facts::{FactsCommand, run_facts};
use crate::knowledge::{KnowledgeCommand, run_knowledge};
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
    /// Which install to act on: `release` (the default), `dev`, or any other flavour id.
    ///
    /// Equivalent to `ADI_FLAVOR`, and composes with the per-field `ADI_*` overrides the same
    /// way. Without it every command addresses the real install — which is the right default,
    /// and the reason it is spelled out rather than inferred from anything.
    #[arg(long, global = true, value_name = "ID")]
    flavor: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the resolved flavour — which install this invocation would act on.
    ///
    /// `--env` emits shell-quoted assignments, which is how the macOS build learns the app
    /// name and bundle id: the presets then have exactly one definition, in Rust, instead of
    /// a second copy in a build script that nothing keeps in step.
    Flavor {
        #[arg(long)]
        json: bool,
        #[arg(long, conflicts_with = "json")]
        env: bool,
    },
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
    /// Goal commands: what a conversation is for, and the two ways it ends.
    Goals {
        #[command(subcommand)]
        command: GoalsCommand,
    },
    /// Trigger commands (background code blocks fired by webhooks & co.).
    Triggers {
        #[command(subcommand)]
        command: TriggersCommand,
    },
    /// Knowledge base commands: scoped collections of text notes, embedded so they can be
    /// searched by meaning. Global, per-project, or per-agent.
    Knowledge {
        /// Act as this agent, so the isolation levels apply as they would to its runs. Without
        /// it (and `--as-project`) the CLI is the owner of the store and reaches everything.
        #[arg(long, value_name = "AGENT")]
        as_agent: Option<String>,
        /// Act as somebody working in this project.
        #[arg(long, value_name = "PROJECT")]
        as_project: Option<String>,
        /// Run as the owner of the store regardless of who the environment says you are — the one
        /// way to write into another agent's memory. Overrides `--as-agent` / `--as-project`.
        #[arg(long)]
        root: bool,
        #[command(subcommand)]
        command: KnowledgeCommand,
    },
    /// Fact base commands: plain sentences an agent writes in bulk, and a graph that makes
    /// anything built on a changed fact go stale. Global, per-project, or per-agent.
    Facts {
        /// The base to work on. Default: `global/default`, or `$ADI_FACTS_BASE`.
        #[arg(long, value_name = "BASE")]
        base: Option<String>,
        /// Act as this agent, so the isolation levels apply as they would to its runs.
        #[arg(long, value_name = "AGENT")]
        as_agent: Option<String>,
        /// Act as somebody working in this project.
        #[arg(long, value_name = "PROJECT")]
        as_project: Option<String>,
        /// Run as the owner of the store regardless of who the environment says you are.
        /// Overrides `--as-agent` / `--as-project`.
        #[arg(long)]
        root: bool,
        #[command(subcommand)]
        command: FactsCommand,
    },
    /// Code index commands: index a project's source and search it by meaning, name, or path.
    Indexer {
        #[command(subcommand)]
        command: IndexerCommand,
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
    /// Internal: serve one conversation's ADI tools to a Claude engine over MCP on stdio. Spawned
    /// by that engine's CLI, which the runner points at this command; not for direct use.
    #[command(hide = true)]
    Mcp {
        /// The agent whose conversation these tools belong to.
        #[arg(long)]
        agent: String,
        /// The session (run) id whose shell and awaits these tools act on.
        #[arg(long)]
        session: String,
        /// The run's own directory — what relative paths resolve against.
        #[arg(long, value_name = "PATH")]
        dir: String,
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

/// Print the resolved flavour, for a human, a script, or `jq`.
fn print_flavor(json: bool, env: bool) {
    let flavour = adi_config::Flavor::current();
    if json {
        match serde_json::to_string_pretty(flavour) {
            Ok(text) => println!("{text}"),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    } else if env {
        for (key, value) in flavour.env() {
            // Single-quoted, with embedded quotes escaped the POSIX way, so a caller can
            // `eval` this without an app name containing a space splitting into two words.
            println!("{key}='{}'", value.replace('\'', r"'\''"));
        }
    } else {
        println!("flavor          {}", flavour.id);
        println!("app             {}", flavour.app_name);
        println!("bundle id       {}", flavour.bundle_id);
        println!("domain          .{}", flavour.domain);
        println!("store           ~/{}/mono", flavour.dir_name);
        println!("labels          {}.*", flavour.label_prefix);
        println!("resolver        127.0.0.1:{}", flavour.resolver_port);
        println!("front door      {}:80", flavour.frontdoor_addr);
        println!("supervisor      127.0.0.1:{}", flavour.supervisor_port);
        println!("auto-update     {}", flavour.auto_update);
    }
}

fn main() {
    let cli = Cli::parse();
    // Before anything else: every path, label, port and hostname below is derived from the
    // flavour, and it resolves itself from the environment the first time it is asked. Pinning
    // has to happen while nothing has asked yet.
    if let Some(id) = cli.flavor.as_deref() {
        if let Err(active) = adi_config::Flavor::pin(adi_config::Flavor::for_id(id)) {
            eprintln!("error: --flavor came too late; already running as '{}'", active.id);
            std::process::exit(1);
        }
    }
    let adi = Adi::new();
    match cli.command {
        Command::Flavor { json, env } => print_flavor(json, env),
        Command::Up => adi.ensure_enabled(),
        Command::Enable => adi.enable(),
        Command::Disable => adi.disable(),
        Command::Status { json } => print_report(&adi.report(), json),
        Command::Dns { command } => match command {
            DnsCommand::Enable => adi.dns().enable(),
            DnsCommand::Disable => adi.dns().disable(),
            DnsCommand::Status { json } => print_service(&adi.dns().report(), json),
            DnsCommand::InstallRoute => adi.dns().install_route(),
            DnsCommand::GrantDns => adi.dns().install_dns_route(),
            DnsCommand::GrantNetwork => adi.dns().install_front_door(),
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
        Command::Goals { command } => {
            if let Err(e) = run_goals(adi, command) {
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
        Command::Knowledge {
            as_agent,
            as_project,
            root,
            command,
        } => {
            if let Err(e) = run_knowledge(adi, as_agent, as_project, root, command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        // No `adi` facade, unlike `knowledge`: the fact store carries an injectable classifier,
        // and putting it on the facade would make `adi-app` — which links that facade — carry a
        // blocking HTTP client it never calls.
        Command::Facts {
            base,
            as_agent,
            as_project,
            root,
            command,
        } => {
            if let Err(e) = run_facts(base, as_agent, as_project, root, command) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        // Like `mesh`, no `adi` facade: an index is state under the project, not the platform.
        Command::Indexer { command } => {
            if let Err(e) = run_indexer(command) {
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
        // stdout is the MCP transport, so a failure cannot be reported there — only on stderr,
        // which the engine's CLI surfaces as the server's own log.
        Command::Mcp {
            agent,
            session,
            dir,
        } => {
            if let Err(e) = adi
                .agents()
                .serve_mcp(&agent, &session, std::path::Path::new(&dir))
            {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
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
    fn the_indexer_group_is_reachable_from_the_top_level() {
        // Its own argv surface is tested in `indexer.rs`; this pins the wiring.
        let cli = Cli::try_parse_from(["adi-mono", "indexer", "status", "--json"]).expect("parses");
        assert!(matches!(
            cli.command,
            Command::Indexer {
                command: IndexerCommand::Status { json: true, .. }
            }
        ));
        assert!(Cli::try_parse_from(["adi-mono", "indexer"]).is_err());
    }

    #[test]
    fn the_knowledge_group_is_reachable_from_the_top_level() {
        // Its own argv surface is tested in `knowledge.rs`; this pins the wiring, including the
        // two identity flags — which sit on the group, before the subcommand.
        let cli = Cli::try_parse_from([
            "adi-mono",
            "knowledge",
            "--as-agent",
            "solver",
            "search",
            "how do I deploy",
        ])
        .expect("parses");
        assert!(matches!(
            cli.command,
            Command::Knowledge {
                as_agent: Some(ref a),
                as_project: None,
                root: false,
                command: KnowledgeCommand::Search { .. },
            } if a.as_str() == "solver"
        ));
        assert!(Cli::try_parse_from(["adi-mono", "knowledge"]).is_err());
    }

    #[test]
    fn the_facts_group_is_reachable_from_the_top_level() {
        // Its own argv surface is tested in `facts.rs`; this pins the wiring, including that
        // `--base` and the identity flags sit on the group, before the verb.
        let cli = Cli::try_parse_from([
            "adi-mono",
            "facts",
            "--base",
            "project:acme/default",
            "--as-agent",
            "solver",
            "stale",
        ])
        .expect("parses");
        assert!(matches!(
            cli.command,
            Command::Facts {
                base: Some(ref b),
                as_agent: Some(ref a),
                root: false,
                command: FactsCommand::Stale,
                ..
            } if b.as_str() == "project:acme/default" && a.as_str() == "solver"
        ));
        assert!(Cli::try_parse_from(["adi-mono", "facts"]).is_err());
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
