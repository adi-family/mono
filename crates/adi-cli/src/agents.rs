//! The `agents` command group: the agent-definition subcommand surface and its dispatch
//! over the shared agent-definition store.

use adi_core::{Adi, AgentManifest, AgentSummaryArguments, Launch, StoredAgent};
use clap::Subcommand;

use crate::format::{clean, clean_required, clean_tags, parse_arguments, print_json};

// `Save` carries the whole definition's worth of flags, dwarfing the name-only variants; a
// one-shot CLI enum, so the size gap costs nothing worth boxing over.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum AgentsCommand {
    /// List agent definitions.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one agent definition.
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Create or replace an agent definition.
    Save {
        name: String,
        /// The `executor:what` backend, e.g. `tmux:claude`, `process:codex`,
        /// `harness:claude-sdk`, `harness:adi`, `wasm:loop-script`.
        #[arg(long)]
        backend: String,
        #[arg(long)]
        system_prompt: Option<String>,
        /// CLI command groups this agent may use, stored as the manifest's command scope.
        #[arg(long = "command-scope")]
        command_scope: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        temperature: Option<f64>,
        #[arg(long)]
        max_turns: Option<u32>,
        /// Repeatable; comma-separated values are also accepted.
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        starred: bool,
        /// The project to file the agent under (its id); omit for a global agent.
        #[arg(long)]
        project: Option<String>,
        /// An adi tool id to enable for this agent (its own `.bin`). Repeatable; comma-separated
        /// values are also accepted. Distinct from `--command-scope` (the LLM's allowed tools).
        #[arg(long = "tool")]
        tools: Vec<String>,
        /// Repeatable key=value backend argument. Objects and arrays may be supplied as JSON.
        #[arg(long = "argument", visible_alias = "extra")]
        arguments: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Launch an agent in its backend. Tmux executors open a detached interactive session,
    /// process executors run a headless CLI in the background, and `wasm:*` agents dispatch
    /// synchronously.
    Run {
        name: String,
        /// The task sent to a process backend or wasm handler (ignored by tmux backends).
        #[arg(short, long, default_value = "run")]
        message: String,
        /// The trigger handler to dispatch into (wasm backends only); defaults to the
        /// agent's first subscription.
        #[arg(long)]
        handler: Option<String>,
    },
    /// Stop a running agent using its executor's lifecycle.
    Stop { name: String },
    /// Delete an agent definition.
    Rm { name: String },
    /// Delete an agent definition.
    Delete { name: String },
}

/// Dispatch an `agents` subcommand over the shared agent-definition store.
pub(crate) fn run_agents(adi: Adi, command: AgentsCommand) -> Result<(), String> {
    let store = adi.agents();
    match command {
        AgentsCommand::List { json } => {
            let agents = store.list().map_err(|e| e.to_string())?;
            if json {
                print_json(&agents);
            } else if agents.is_empty() {
                println!("No agents registered.");
            } else {
                for agent in &agents {
                    print_agent(agent);
                }
            }
        }
        AgentsCommand::Show { name, json } => {
            let agent = store
                .get(&name)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such agent: {name}"))?;
            if json {
                print_json(&agent);
            } else {
                print_agent(&agent);
            }
        }
        AgentsCommand::Save {
            name,
            backend,
            system_prompt,
            command_scope,
            model,
            permission_mode,
            temperature,
            max_turns,
            tags,
            starred,
            project,
            tools,
            arguments,
            json,
        } => {
            let backend = clean_required("backend", backend)?;
            let mut arguments = parse_arguments(arguments)?;
            if let Some(value) = clean(system_prompt) {
                arguments.insert("system_prompt".into(), value.into());
            }
            if let Some(value) = clean(command_scope) {
                arguments.insert("tools".into(), value.into());
            }
            if let Some(value) = clean(model) {
                arguments.insert("model".into(), value.into());
            }
            if let Some(value) = clean(permission_mode) {
                arguments.insert("permission_mode".into(), value.into());
            }
            if let Some(value) = temperature {
                arguments.insert("temperature".into(), value.into());
            }
            if let Some(value) = max_turns {
                arguments.insert("max_turns".into(), value.into());
            }
            let manifest = AgentManifest {
                backend: backend.into(),
                arguments,
                tags: clean_tags(tags),
                starred,
                project: clean(project),
                bin_tools: clean_tags(tools),
                created_at: 0,
                updated_at: 0,
            };
            let agent = store.save(&name, manifest).map_err(|e| e.to_string())?;
            if json {
                print_json(&agent);
            } else {
                println!("Saved agent {}.", agent.name);
                print_agent(&agent);
            }
        }
        AgentsCommand::Run {
            name,
            message,
            handler,
        } => {
            let is_wasm = store
                .get(&name)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such agent: {name}"))?
                .manifest
                .executor()
                == "wasm";
            if is_wasm {
                let outcome = store
                    .run_wasm(&name, handler.as_deref(), &message)
                    .map_err(|e| e.to_string())?;
                println!(
                    "Dispatched to agent {} via {} (llm turns: {}, tokens: {}/{}).",
                    outcome.employee,
                    outcome.subscription,
                    outcome.turns,
                    outcome.input_tokens,
                    outcome.output_tokens,
                );
            } else {
                let launch = store
                    .run_with_message(&name, &message)
                    .map_err(|e| e.to_string())?;
                match launch {
                    Launch::Tmux { command, session } => {
                        println!("Started agent {name} in tmux session {session}.");
                        println!("  command: {command}");
                        println!("  attach:  tmux attach -t {session}");
                    }
                    Launch::Process {
                        command,
                        pid,
                        log,
                        run_id,
                    } => {
                        println!("Started agent {name} as background process {pid}.");
                        println!("  run:     {run_id}");
                        println!("  command: {command}");
                        println!("  log:     {}", log.display());
                    }
                }
            }
        }
        AgentsCommand::Stop { name } => {
            if store.stop(&name).map_err(|e| e.to_string())? {
                println!("Stopped agent {name}.");
            } else {
                println!("Agent {name} wasn't running.");
            }
        }
        AgentsCommand::Rm { name } | AgentsCommand::Delete { name } => {
            if store.delete(&name).map_err(|e| e.to_string())? {
                println!("Deleted agent {name}.");
            } else {
                println!("No such agent: {name}.");
            }
        }
    }
    Ok(())
}

/// Print an agent definition in the compact human CLI format.
fn print_agent(agent: &StoredAgent) {
    let arguments = agent
        .manifest
        .typed_arguments::<AgentSummaryArguments>()
        .unwrap_or_default();
    println!(
        "{} — {} [{}]",
        agent.name,
        agent.manifest.backend,
        agent.manifest.executor()
    );
    if let Some(model) = arguments.model {
        println!("  model: {model}");
    }
    if let Some(project) = &agent.manifest.project {
        println!("  project: {project}");
    }
    if let Some(tools) = arguments.tools.filter(|tools| !tools.trim().is_empty()) {
        println!("  commands: {tools}");
    }
    if !agent.manifest.bin_tools.is_empty() {
        println!("  tools (.bin): {}", agent.manifest.bin_tools.join(", "));
    }
    if !agent.manifest.tags.is_empty() {
        println!("  tags: {}", agent.manifest.tags.join(", "));
    }
    if agent.manifest.starred {
        println!("  starred");
    }
}
