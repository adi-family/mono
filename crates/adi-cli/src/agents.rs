//! The `agents` command group: the agent-definition subcommand surface and its dispatch
//! over the shared agent-definition store.

use std::collections::BTreeMap;

use adi_core::{
    Adi, AgentManifest, Agents, AgentSummaryArguments, AgentsError, Launch, SecretAttachment,
    StoredAgent,
};
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
        /// The `executor:what` backend, e.g. `pty:claude`, `process:codex`,
        /// `harness:claude-sdk`, `harness:adi`.
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
        /// A secret to attach to this agent (injected into its runs as an env var under its name).
        /// Give `NAME` for a global secret or `PROJECT/NAME` for a project-scoped one. Repeatable;
        /// comma-separated values are also accepted. Only attached secrets are injected — an
        /// explicit allowlist.
        #[arg(long = "secret")]
        secrets: Vec<String>,
        /// A directory to put on this agent's run `PATH`, ahead of the machine's own — how an agent
        /// pins a toolchain the default `PATH` doesn't point at. `~` and `$HOME` are expanded at
        /// launch. Repeatable. Omit every `--path` to leave an existing agent's dirs as they are;
        /// pass `--no-path` to clear them.
        #[arg(long = "path")]
        path: Vec<String>,
        /// Clear the agent's extra `PATH` dirs (see `--path`).
        #[arg(long, conflicts_with = "path")]
        no_path: bool,
        /// An environment variable for this agent's runs, as `KEY=VALUE`. Repeatable. Omit every
        /// `--env` to leave an existing agent's variables alone; pass `--no-env` to clear them.
        /// `PATH` is rejected here — it is built from `--path`.
        #[arg(long = "env")]
        env: Vec<String>,
        /// Clear the agent's extra environment variables (see `--env`).
        #[arg(long, conflicts_with = "env")]
        no_env: bool,
        /// This agent runs with nobody watching, so it may not stop to ask: the `Ask` tool refuses
        /// and tells the run to decide for itself and say what it assumed. For a trigger's agent, a
        /// scheduled sweep — anything whose questions nobody would see.
        #[arg(long)]
        unattended: bool,
        /// Repeatable key=value backend argument. Objects and arrays may be supplied as JSON.
        #[arg(long = "argument", visible_alias = "extra")]
        arguments: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Launch an agent in its backend. Pty executors open an interactive session and
    /// process executors run a headless CLI in the background.
    Run {
        name: String,
        /// The task sent to a process backend (ignored by pty backends).
        #[arg(short, long, default_value = "run")]
        message: String,
        /// Where this run starts, overriding the agent's own `working_dir` and its project
        /// directory. For pointing one agent at a different target each run.
        #[arg(long, value_name = "PATH")]
        dir: Option<String>,
        /// Launch even when as many runs are already live as the store allows. The deliberate
        /// override of the concurrency limit — for a human who wants this one now.
        #[arg(long)]
        force: bool,
        /// Block until the run finishes, print its answer, and exit with its status.
        ///
        /// What turns a launch into something another program can be composed out of: a shell
        /// script, a CI step, or an agent's own `Bash` — which is how one agent waits for another
        /// without either of them learning a new verb. Not for interactive backends: a pty run has
        /// no ending to wait for, only a pane somebody is looking at.
        #[arg(long)]
        wait: bool,
    },
    /// Show, or set, how many agent runs may be live at once — overall, or (with `--project`) for
    /// one project's agents. The caps bind every launch the platform makes on its own (a trigger
    /// firing, a queued chat turn); a human overrides them per run with `agents run --force`.
    Limit {
        /// The new cap. `0` lifts it (for a project: clears its own cap). Omit to read the
        /// current ones.
        max: Option<u32>,
        /// Set this project's cap instead of the global one. A project cap narrows the global
        /// number, it never lifts it.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List the questions runs are waiting on a person to answer.
    ///
    /// A run that needs a decision writes it down and ends its turn, so nothing is held open —
    /// which also means nothing will move until somebody answers. This is what is waiting.
    Questions {
        /// Only this agent's questions.
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Answer the question a conversation is waiting on, and let it carry on.
    ///
    /// Give one reply per question, in the order they were asked (`agents questions` prints them
    /// numbered). One reply for a single-question ask is the ordinary case.
    Answer {
        /// The agent whose conversation is waiting.
        name: String,
        /// The conversation id — what `agents questions` prints as `conv`.
        conv: String,
        /// The answers, in the order the questions were asked.
        #[arg(required = true)]
        replies: Vec<String>,
        /// Answer this specific ask, refusing if it has been settled since. Without it, whatever is
        /// pending now is answered.
        #[arg(long)]
        ask: Option<String>,
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
            secrets,
            path,
            no_path,
            env,
            no_env,
            unattended,
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
            // A save states the whole agent, so the run environment has to be carried over when
            // this invocation says nothing about it — otherwise every `agents save` on an existing
            // agent would silently strip the toolchain it was pinned to.
            let stored = store.get(&name).ok().flatten().map(|a| a.manifest);
            let manifest = AgentManifest {
                backend: backend.into(),
                arguments,
                tags: clean_tags(tags),
                starred,
                project: clean(project),
                bin_tools: clean_tags(tools),
                secrets: parse_secret_attachments(secrets),
                path: if no_path {
                    Vec::new()
                } else if path.is_empty() {
                    stored.as_ref().map(|m| m.path.clone()).unwrap_or_default()
                } else {
                    // Not `clean_tags`: a directory may legitimately contain a comma, so each
                    // `--path` is one dir, never a comma-separated list.
                    path.into_iter()
                        .map(|dir| dir.trim().to_string())
                        .filter(|dir| !dir.is_empty())
                        .collect()
                },
                env: if no_env {
                    BTreeMap::new()
                } else if env.is_empty() {
                    stored.as_ref().map(|m| m.env.clone()).unwrap_or_default()
                } else {
                    parse_env_vars(env)?
                },
                unattended,
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
            dir,
            force,
            wait,
        } => {
            let launch = if force {
                store.force_run_in(&name, &message, dir.as_deref())
            } else {
                store.run_in(&name, &message, dir.as_deref())
            }
            .map_err(|e| match e {
                // The refusal is the one error a human can act on from here, so it says how.
                AgentsError::TooManyRunning { .. } => {
                    format!("{e}: re-run with --force to launch it anyway")
                }
                other => other.to_string(),
            })?;
            match launch {
                Launch::Pty { command, session } => {
                    if wait {
                        return Err(
                            "--wait needs a run that ends; a pty agent opens a pane instead"
                                .to_string(),
                        );
                    }
                    println!("Started agent {name} in session {session}.");
                    println!("  command: {command}");
                    println!("  view:    in the control panel's live view (no external attach)");
                }
                Launch::Process {
                    command,
                    pid,
                    log,
                    run_id,
                } => {
                    if wait {
                        return await_run(&store, &name, &run_id);
                    }
                    println!("Started agent {name} as background process {pid}.");
                    println!("  run:     {run_id}");
                    println!("  command: {command}");
                    println!("  log:     {}", log.display());
                }
            }
        }
        AgentsCommand::Limit {
            max,
            project,
            json,
        } => {
            let limits = match (max, project.as_deref()) {
                (Some(max), Some(project)) => store
                    .set_project_limit(project, max)
                    .map_err(|e| e.to_string())?,
                (Some(max_concurrent_runs), None) => {
                    let mut limits = store.limits();
                    limits.max_concurrent_runs = max_concurrent_runs;
                    store.set_limits(limits).map_err(|e| e.to_string())?
                }
                (None, _) => store.limits(),
            };
            let load = store.run_load();
            if json {
                print_json(&serde_json::json!({
                    "max_concurrent_runs": limits.max_concurrent_runs,
                    "running": load.total(),
                    "projects": limits.projects.iter().map(|(id, max)| serde_json::json!({
                        "project": id,
                        "max_concurrent_runs": max,
                        "running": load.in_project(id),
                    })).collect::<Vec<_>>(),
                }));
            } else {
                if limits.max_concurrent_runs == 0 {
                    println!("No overall limit ({} running).", load.total());
                } else {
                    println!(
                        "At most {} agent runs at once ({} running).",
                        limits.max_concurrent_runs,
                        load.total()
                    );
                }
                for (id, max) in &limits.projects {
                    println!(
                        "  {id}: at most {max} ({} running)",
                        load.in_project(id)
                    );
                }
            }
        }
        AgentsCommand::Questions { agent, json } => {
            let agent = clean(agent);
            let waiting: Vec<_> = store
                .pending_questions()
                .into_iter()
                .filter(|ask| agent.as_ref().is_none_or(|name| ask.agent == *name))
                .collect();
            if json {
                print_json(&waiting);
            } else if waiting.is_empty() {
                println!("Nothing is waiting on you.");
            } else {
                for ask in &waiting {
                    print_ask(ask);
                }
            }
        }
        AgentsCommand::Answer {
            name,
            conv,
            replies,
            ask,
        } => {
            let ask = clean(ask);
            let sent = store
                .answer(&name, &conv, ask.as_deref(), &replies)
                .map_err(|e| e.to_string())?;
            match sent {
                adi_core::Sent::Started(_) => println!("Answered — {name} is working on it."),
                adi_core::Sent::Queued { place } => println!(
                    "Answered — {place} in line behind what {name} is already saying."
                ),
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

/// Print one waiting ask: who is blocked, on what, and the numbered questions to answer in order.
///
/// Numbered because `agents answer` takes the replies positionally — the numbers here are the order
/// to type them in, which is the whole reason a multi-question ask is legible from a terminal at all.
fn print_ask(ask: &adi_core::Ask) {
    println!("{} · conv {}", ask.agent, ask.conv);
    println!("  ask {}", ask.id);
    if !ask.note.is_empty() {
        println!("  {}", ask.note);
    }
    for (index, question) in ask.questions.iter().enumerate() {
        let header = if question.header.is_empty() {
            String::new()
        } else {
            format!("[{}] ", question.header)
        };
        println!("  {}. {header}{}", index + 1, question.question);
        for option in &question.options {
            match option.description.is_empty() {
                true => println!("       - {}", option.label),
                false => println!("       - {} — {}", option.label, option.description),
            }
        }
    }
    println!(
        "  answer with: adi-mono agents answer {} {} {}",
        ask.agent,
        ask.conv,
        ask.questions
            .iter()
            .map(|_| "'…'")
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!();
}

/// Parse `--secret` values into attachments. Each value is a comma-separated list of
/// `NAME` (global) or `PROJECT/NAME` (project-scoped) references; blanks are dropped.
/// Parse `--env KEY=VALUE` flags into the manifest's env table. The value is everything after the
/// first `=`, so it may contain one itself. `PATH` is rejected rather than accepted-and-ignored:
/// it is assembled at launch from `--path`, so honouring it here would be a lie.
fn parse_env_vars(values: Vec<String>) -> Result<BTreeMap<String, String>, String> {
    let mut vars = BTreeMap::new();
    for value in values {
        let Some((key, value)) = value.split_once('=') else {
            return Err(format!("--env expects KEY=VALUE, got: {value}"));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err("--env needs a variable name before the `=`".to_string());
        }
        if key == "PATH" {
            return Err("--env cannot set PATH; use --path to add directories to it".to_string());
        }
        vars.insert(key.to_string(), value.trim().to_string());
    }
    Ok(vars)
}

fn parse_secret_attachments(values: Vec<String>) -> Vec<SecretAttachment> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .filter_map(|token| {
            let token = token.trim();
            if token.is_empty() {
                return None;
            }
            let attachment = match token.split_once('/') {
                Some((project, name)) => SecretAttachment {
                    project: Some(project.trim().to_string()),
                    name: name.trim().to_string(),
                },
                None => SecretAttachment {
                    project: None,
                    name: token.to_string(),
                },
            };
            (!attachment.name.is_empty()).then_some(attachment)
        })
        .collect()
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
    if !agent.manifest.secrets.is_empty() {
        let refs: Vec<String> = agent
            .manifest
            .secrets
            .iter()
            .map(|s| match &s.project {
                Some(project) => format!("{project}/{}", s.name),
                None => s.name.clone(),
            })
            .collect();
        println!("  secrets: {}", refs.join(", "));
    }
    if !agent.manifest.path.is_empty() {
        println!("  path: {}", agent.manifest.path.join(", "));
    }
    if !agent.manifest.env.is_empty() {
        // Names only: an agent's env table is ordinary config, but printing values into a terminal
        // (and its scrollback) is not what `agents show` is for.
        let names: Vec<&str> = agent.manifest.env.keys().map(String::as_str).collect();
        println!("  env: {}", names.join(", "));
    }
    if !agent.manifest.tags.is_empty() {
        println!("  tags: {}", agent.manifest.tags.join(", "));
    }
    if agent.manifest.starred {
        println!("  starred");
    }
}

/// Block until `run_id` finishes, print what the agent answered, and exit with the run's own
/// status — the whole of `--wait`.
///
/// The wait is a poll rather than a subscription because a run's ending is a property of a process
/// this one did not spawn: the launch is detached, so there is no child here to wait on, and the
/// store's liveness answer is the same one every other reader uses.
///
/// The exit status is the *agent's*, not this command's: a turn the engine reported as failed exits
/// non-zero, so `adi-mono agents run … --wait && deploy` means what it looks like. A run that
/// answered nothing is a failure too — there is no result to have composed anything out of.
fn await_run(store: &Agents, name: &str, run_id: &str) -> Result<(), String> {
    /// Long enough that a multi-minute run costs a handful of store reads, short enough that a
    /// quick one is not held up by the polling itself.
    const LOOK_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

    let agent = store
        .get(name)
        .map_err(|e: AgentsError| e.to_string())?
        .ok_or_else(|| format!("no agent named {name}"))?;

    while store.peek_run(&agent, run_id).running {
        std::thread::sleep(LOOK_EVERY);
    }

    // The answer is the last thing the agent said, which is what a caller composing on this wants —
    // not the log, which carries the engine's own event stream around it.
    let answer = store
        .transcript(&agent, run_id)
        .into_iter()
        .rfind(|turn| turn.role == "assistant");

    match answer {
        Some(turn) => {
            let text = turn.text.trim();
            if !text.is_empty() {
                println!("{text}");
            }
            // `is_error` is the engine's own verdict on the turn, so a run that failed says so in
            // the only way a shell reads.
            if turn.metrics.as_ref().is_some_and(|m| m.is_error) || text.is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
        None => Err(format!("run {run_id} finished without answering")),
    }
}
