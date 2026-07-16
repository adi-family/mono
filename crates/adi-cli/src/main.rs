//! adi-mono — the adi platform CLI: a thin argv adapter over `adi-core`'s command
//! surface where every subcommand maps 1:1 to a method call, so the GUI can trigger
//! platform actions by running this binary.

use std::collections::BTreeMap;

use adi_core::{
    Adi, Agent, AgentManifest, EffectiveStatus, Project, Report, RunOutcome, Service,
    ServiceReport, TaskPatch, TaskStatus, TaskView, Trigger, TriggerManifest, Updater,
};
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

// Carries only flags, so it's `Copy` (which also satisfies pedantic's pass-by-value lint).
#[derive(Debug, Clone, Copy, Subcommand)]
enum UpdateCommand {
    /// Fetch the release manifest and compare against the installed version (no install).
    Check {
        #[arg(long)]
        json: bool,
    },
    /// Download, verify, and install the latest version if newer, then restart services.
    Run {
        /// Reinstall even when the published version isn't newer.
        #[arg(long)]
        force: bool,
        /// Swap the bundle but leave running services on the old binaries.
        #[arg(long)]
        no_restart: bool,
        /// Exit 0 on errors too (offline is normal) — what the background agent runs.
        #[arg(long)]
        quiet: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show the updater's persisted last check/install record.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Enable the background auto-update agent (periodic check + install).
    Enable,
    /// Disable the background auto-update agent.
    Disable,
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
        /// The registered project to nest this one under (a sub-project); omit for top-level.
        #[arg(long)]
        parent: Option<String>,
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

#[derive(Debug, Subcommand)]
enum TasksCommand {
    /// List tasks, optionally filtered.
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        /// Stored status: open, done, archived.
        #[arg(long)]
        status: Option<String>,
        /// Computed status: ready, blocked, done, archived.
        #[arg(long)]
        effective: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create a task.
    Add {
        title: String,
        #[arg(long)]
        details: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show one task.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Edit task fields. Pass an empty string to clear an optional field.
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        details: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        assignee: Option<String>,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Mark a task done.
    Complete {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Mark a task done.
    Done {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Archive a task.
    Archive {
        id: String,
        #[arg(long)]
        cascade: bool,
        #[arg(long)]
        json: bool,
    },
    /// Reopen a done or archived task.
    Reopen {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Permanently delete a task.
    Rm { id: String },
    /// Permanently delete a task.
    Delete { id: String },
}

// `Save` carries the whole definition's worth of flags, dwarfing the name-only variants; a
// one-shot CLI enum, so the size gap costs nothing worth boxing over.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
enum AgentsCommand {
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
        /// Repeatable key=value backend-specific parameter.
        #[arg(long = "extra")]
        extra: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Launch an agent in its backend: the engine CLI starts detached in an
    /// `adi-agent-<name>` tmux session (tmux executors only for now).
    Run { name: String },
    /// Stop a running agent by killing its `adi-agent-<name>` tmux session.
    Stop { name: String },
    /// Delete an agent definition.
    Rm { name: String },
    /// Delete an agent definition.
    Delete { name: String },
}

#[derive(Debug, Subcommand)]
enum TriggersCommand {
    /// List trigger definitions.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one trigger definition.
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Create or replace a trigger definition.
    Save {
        name: String,
        /// The event source that fires it: webhook, telegram, cron, or manual.
        #[arg(long)]
        kind: String,
        /// The shell code block spawned (detached) when the trigger fires.
        #[arg(long)]
        code: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// The project to file the trigger under (its id); omit for a global trigger.
        #[arg(long)]
        project: Option<String>,
        /// Save the trigger disabled (its external source won't fire it).
        #[arg(long)]
        disabled: bool,
        /// Repeatable key=value kind-specific setting (`secret`, `schedule`, `token_env`, `chat_id`, …).
        #[arg(long = "extra")]
        extra: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Enable a trigger (its external source may fire it again).
    Enable { name: String },
    /// Disable a trigger (keeps the definition; the external source refuses to fire it).
    Disable { name: String },
    /// Fire a trigger by hand: spawn its code block detached, output to its log.
    Fire { name: String },
    /// Print the tail of a trigger's most recent fire log.
    Log { name: String },
    /// Delete a trigger definition.
    Rm { name: String },
    /// Delete a trigger definition.
    Delete { name: String },
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

/// Dispatch an `update` subcommand over the adi-core update facade.
fn run_update(adi: Adi, command: UpdateCommand) {
    match command {
        UpdateCommand::Check { json } => match adi.update().check() {
            Ok(check) => {
                if json {
                    print_json(&check);
                } else {
                    println!("Installed: {}", check.installed);
                    println!("Latest:    {}", check.latest);
                    if check.update_available {
                        println!("Update available — run `adi-mono update run` to install.");
                    } else {
                        println!("Up to date.");
                    }
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
        UpdateCommand::Run {
            force,
            no_restart,
            quiet,
            json,
        } => match adi.update().run(force, !no_restart) {
            Ok(outcome) => {
                if json {
                    print_json(&outcome);
                } else {
                    match outcome {
                        RunOutcome::UpToDate { installed, .. } => {
                            println!("Up to date ({installed}).");
                        }
                        RunOutcome::Installed {
                            from,
                            to,
                            restarted,
                        } => {
                            println!(
                                "Updated {from} → {to}{}.",
                                if restarted {
                                    "; services restarted"
                                } else {
                                    " (services not restarted)"
                                }
                            );
                        }
                    }
                }
            }
            Err(e) => {
                // In quiet (background-agent) mode an unreachable manifest is routine —
                // log it and exit 0 so launchd doesn't treat the tick as a failure.
                eprintln!("error: {e}");
                if !quiet {
                    std::process::exit(1);
                }
            }
        },
        UpdateCommand::Status { json } => {
            let state = adi.update().state();
            if json {
                print_json(&state);
            } else if state.last_check_unix.is_none() {
                println!("Never checked for updates.");
            } else {
                println!(
                    "{} — installed {}, latest {} (last check {} unix)",
                    state.last_outcome.as_deref().unwrap_or("unknown"),
                    state.installed_version.as_deref().unwrap_or("?"),
                    state.latest_version.as_deref().unwrap_or("?"),
                    state.last_check_unix.unwrap_or(0),
                );
                if let Some(err) = &state.last_error {
                    println!("  last error: {err}");
                }
            }
        }
        UpdateCommand::Enable => Updater::new().enable(),
        UpdateCommand::Disable => Updater::new().disable(),
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
            parent,
            json,
        } => {
            let project = store.create(&id, name, description, parent)?;
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

/// Dispatch a `tasks` subcommand over the shared task store.
fn run_tasks(adi: Adi, command: TasksCommand) -> Result<(), String> {
    let store = adi.tasks();
    match command {
        TasksCommand::List {
            project,
            tag,
            status,
            effective,
            json,
        } => {
            let status = parse_task_status_opt(status)?;
            let effective = parse_effective_status_opt(effective)?;
            let mut tasks = store
                .list(project, tag, status, effective)
                .map_err(|e| e.to_string())?;
            tasks.sort_by(task_order);
            if json {
                print_json(&tasks);
            } else if tasks.is_empty() {
                println!("No tasks.");
            } else {
                for task in &tasks {
                    print_task(task);
                }
            }
        }
        TasksCommand::Add {
            title,
            details,
            project,
            tag,
            parent,
            json,
        } => {
            let task = store
                .create(title, details, project, tag, parent)
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&task);
            } else {
                println!("Created task {}.", task.task.id);
                print_task(&task);
            }
        }
        TasksCommand::Show { id, json } => {
            let task = store.get(&id).map_err(|e| e.to_string())?;
            if json {
                print_json(&task);
            } else {
                print_task(&task);
            }
        }
        TasksCommand::Edit {
            id,
            title,
            details,
            tag,
            assignee,
            parent,
            json,
        } => {
            let task = store
                .update(
                    &id,
                    TaskPatch {
                        title,
                        details,
                        tag,
                        assignee,
                        parent,
                    },
                )
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&task);
            } else {
                println!("Updated task {}.", task.task.id);
                print_task(&task);
            }
        }
        TasksCommand::Complete { id, json } | TasksCommand::Done { id, json } => {
            let task = store.complete(&id).map_err(|e| e.to_string())?;
            if json {
                print_json(&task);
            } else {
                println!("Completed task {}.", task.task.id);
                print_task(&task);
            }
        }
        TasksCommand::Archive { id, cascade, json } => {
            let task = store.archive(&id, cascade).map_err(|e| e.to_string())?;
            if json {
                print_json(&task);
            } else {
                println!("Archived task {}.", task.task.id);
                print_task(&task);
            }
        }
        TasksCommand::Reopen { id, json } => {
            let task = store.reopen(&id).map_err(|e| e.to_string())?;
            if json {
                print_json(&task);
            } else {
                println!("Reopened task {}.", task.task.id);
                print_task(&task);
            }
        }
        TasksCommand::Rm { id } | TasksCommand::Delete { id } => {
            store.delete(&id).map_err(|e| e.to_string())?;
            println!("Deleted task {id}.");
        }
    }
    Ok(())
}

/// Dispatch an `agents` subcommand over the shared agent-definition store.
fn run_agents(adi: Adi, command: AgentsCommand) -> Result<(), String> {
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
            extra,
            json,
        } => {
            let backend = clean_required("backend", backend)?;
            let manifest = AgentManifest {
                backend,
                system_prompt: system_prompt.unwrap_or_default(),
                tools: clean(command_scope).unwrap_or_default(),
                model: clean(model),
                permission_mode: clean(permission_mode),
                temperature,
                max_turns,
                tags: clean_tags(tags),
                starred,
                project: clean(project),
                extra: parse_extra(extra)?,
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
        AgentsCommand::Run { name } => {
            let launch = store.run(&name).map_err(|e| e.to_string())?;
            println!("Started agent {name} in tmux session {}.", launch.session);
            println!("  command: {}", launch.command);
            println!("  attach:  {}", launch.attach);
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

/// Dispatch a `triggers` subcommand over the shared trigger-definition store.
fn run_triggers(adi: Adi, command: TriggersCommand) -> Result<(), String> {
    let store = adi.triggers();
    match command {
        TriggersCommand::List { json } => {
            let triggers = store.list().map_err(|e| e.to_string())?;
            if json {
                print_json(&triggers);
            } else if triggers.is_empty() {
                println!("No triggers registered.");
            } else {
                for trigger in &triggers {
                    print_trigger(&store, trigger);
                }
            }
        }
        TriggersCommand::Show { name, json } => {
            let trigger = store
                .get(&name)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such trigger: {name}"))?;
            if json {
                print_json(&trigger);
            } else {
                print_trigger(&store, &trigger);
                if !trigger.manifest.code.trim().is_empty() {
                    println!("  code: {}", trigger.manifest.code);
                }
            }
        }
        TriggersCommand::Save {
            name,
            kind,
            code,
            description,
            project,
            disabled,
            extra,
            json,
        } => {
            let kind = clean_required("kind", kind)?;
            let manifest = TriggerManifest {
                kind,
                code: code.unwrap_or_default(),
                description: clean(description).unwrap_or_default(),
                enabled: !disabled,
                project: clean(project),
                extra: parse_extra(extra)?,
                created_at: 0,
                updated_at: 0,
            };
            let trigger = store.save(&name, manifest).map_err(|e| e.to_string())?;
            if json {
                print_json(&trigger);
            } else {
                println!("Saved trigger {}.", trigger.name);
                print_trigger(&store, &trigger);
            }
        }
        TriggersCommand::Enable { name } => {
            set_trigger_enabled(&store, &name, true)?;
            println!("Enabled trigger {name}.");
        }
        TriggersCommand::Disable { name } => {
            set_trigger_enabled(&store, &name, false)?;
            println!("Disabled trigger {name}.");
        }
        TriggersCommand::Fire { name } => {
            let firing = store.fire(&name, None).map_err(|e| e.to_string())?;
            println!("Fired trigger {name} (pid {}).", firing.pid);
            println!("  log: {}", firing.log.display());
        }
        TriggersCommand::Log { name } => {
            store
                .get(&name)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such trigger: {name}"))?;
            match store.read_log(&name) {
                Some(output) => print!("{output}"),
                None => println!("Trigger {name} has never fired."),
            }
        }
        TriggersCommand::Rm { name } | TriggersCommand::Delete { name } => {
            if store.delete(&name).map_err(|e| e.to_string())? {
                println!("Deleted trigger {name}.");
            } else {
                println!("No such trigger: {name}.");
            }
        }
    }
    Ok(())
}

/// Flip a trigger's enabled flag by re-saving its manifest (the store preserves `created_at`).
fn set_trigger_enabled(
    store: &adi_core::Triggers,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let trigger = store
        .get(name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no such trigger: {name}"))?;
    let mut manifest = trigger.manifest;
    manifest.enabled = enabled;
    store.save(name, manifest).map_err(|e| e.to_string())?;
    Ok(())
}

/// Print a trigger definition in the compact human CLI format.
fn print_trigger(store: &adi_core::Triggers, trigger: &Trigger) {
    let state = if trigger.manifest.enabled {
        "enabled"
    } else {
        "disabled"
    };
    println!("{} — {} [{state}]", trigger.name, trigger.manifest.kind);
    if !trigger.manifest.description.trim().is_empty() {
        println!("  {}", trigger.manifest.description);
    }
    if let Some(project) = &trigger.manifest.project {
        println!("  project: {project}");
    }
    if !trigger.manifest.extra.is_empty() {
        let extras: Vec<String> = trigger
            .manifest
            .extra
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        println!("  extra: {}", extras.join(" · "));
    }
    if let Some(fired) = store.last_fired(&trigger.name) {
        println!("  last fired: {fired} (unix)");
    }
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
    if let Some(parent) = &project.manifest.parent {
        println!("  parent: {parent}");
    }
}

/// Print a task in the compact human CLI format.
fn print_task(task: &TaskView) {
    println!(
        "{} — {} [{}]",
        task.task.id,
        task.task.title,
        effective_name(task.effective)
    );
    let mut meta = vec![format!("status: {}", status_name(task.task.status))];
    if let Some(project) = &task.task.project {
        meta.push(format!("project: {project}"));
    }
    if let Some(parent) = &task.task.parent {
        meta.push(format!("parent: {parent}"));
    }
    if let Some(tag) = &task.task.tag {
        meta.push(format!("tag: {tag}"));
    }
    if let Some(assignee) = &task.task.assignee {
        meta.push(format!("assignee: {assignee}"));
    }
    if task.children_total > 0 {
        meta.push(format!(
            "subtasks: {}/{} open",
            task.children_open, task.children_total
        ));
    }
    println!("  {}", meta.join(" · "));
    if let Some(details) = &task.task.details {
        println!("  {details}");
    }
}

/// Print an agent definition in the compact human CLI format.
fn print_agent(agent: &Agent) {
    println!(
        "{} — {} [{}]",
        agent.name,
        agent.manifest.backend,
        agent.manifest.executor()
    );
    if let Some(model) = &agent.manifest.model {
        println!("  model: {model}");
    }
    if let Some(project) = &agent.manifest.project {
        println!("  project: {project}");
    }
    if !agent.manifest.tools.trim().is_empty() {
        println!("  commands: {}", agent.manifest.tools);
    }
    if !agent.manifest.tags.is_empty() {
        println!("  tags: {}", agent.manifest.tags.join(", "));
    }
    if agent.manifest.starred {
        println!("  starred");
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

fn parse_task_status_opt(value: Option<String>) -> Result<Option<TaskStatus>, String> {
    value.map(|v| parse_task_status(&v)).transpose()
}

fn parse_task_status(value: &str) -> Result<TaskStatus, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "open" | "pending" | "in_progress" => Ok(TaskStatus::Open),
        "done" => Ok(TaskStatus::Done),
        "archived" | "cancelled" => Ok(TaskStatus::Archived),
        _ => Err(format!(
            "unknown task status {value:?}; expected open, done, or archived"
        )),
    }
}

fn parse_effective_status_opt(value: Option<String>) -> Result<Option<EffectiveStatus>, String> {
    value.map(|v| parse_effective_status(&v)).transpose()
}

fn parse_effective_status(value: &str) -> Result<EffectiveStatus, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "ready" => Ok(EffectiveStatus::Ready),
        "blocked" => Ok(EffectiveStatus::Blocked),
        "done" => Ok(EffectiveStatus::Done),
        "archived" => Ok(EffectiveStatus::Archived),
        _ => Err(format!(
            "unknown effective status {value:?}; expected ready, blocked, done, or archived"
        )),
    }
}

fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Open => "open",
        TaskStatus::Done => "done",
        TaskStatus::Archived => "archived",
    }
}

fn effective_name(status: EffectiveStatus) -> &'static str {
    match status {
        EffectiveStatus::Ready => "ready",
        EffectiveStatus::Blocked => "blocked",
        EffectiveStatus::Done => "done",
        EffectiveStatus::Archived => "archived",
    }
}

fn task_order(a: &TaskView, b: &TaskView) -> std::cmp::Ordering {
    a.task
        .created_at
        .cmp(&b.task.created_at)
        .then_with(|| a.task.id.cmp(&b.task.id))
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn clean_required(name: &str, value: String) -> Result<String, String> {
    clean(Some(value)).ok_or_else(|| format!("{name} is required"))
}

fn clean_tags(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .flat_map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn parse_extra(values: Vec<String>) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for raw in values {
        let (key, value) = raw
            .split_once('=')
            .ok_or_else(|| format!("extra value {raw:?} must be key=value"))?;
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        if !safe_extra_key(key) {
            return Err(format!(
                "invalid extra key {key:?}: use letters, digits, '_' or '-'"
            ));
        }
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn safe_extra_key(key: &str) -> bool {
    key.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
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
