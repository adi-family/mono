//! The `projects` command group, together with its nested `workspace` and `hook`
//! subcommands — they share `project_scope`, so they live in one module.

use adi_core::{Adi, Project};
use clap::Subcommand;

use crate::format::print_json;

#[derive(Debug, Subcommand)]
pub(crate) enum ProjectsCommand {
    /// List registered projects (active only unless `--all`).
    List {
        /// Include archived projects.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Register a new project under a generated UUID id (writes projects/<uuid>/config.toml).
    Add {
        /// The display name; the id is generated, so this is all a new project needs.
        name: String,
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
    /// Workspace commands: the project's working copies, created by its hooks.
    Workspace {
        /// The project id the workspaces belong to.
        project: String,
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Project hook commands: the script files under the project's `.adi/hooks`.
    Hook {
        /// The project id the hooks belong to.
        project: String,
        #[command(subcommand)]
        command: HookCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum WorkspaceCommand {
    /// List the project's workspaces with live status.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Create a workspace: the project's FIRST runs the `init` hook (e.g. git clone),
    /// each ADDITIONAL one the `workspace` hook (e.g. git worktree add). The hook runs
    /// detached; watch it with `projects hook <project> log <name>`.
    Add {
        name: String,
        /// Register at this absolute path instead of `<project>/workspaces/<name>`.
        #[arg(long)]
        path: Option<String>,
        /// Link an existing directory as-is — run no hook.
        #[arg(long)]
        local: bool,
        #[arg(long)]
        json: bool,
    },
    /// Unregister a workspace (never deletes its files).
    Rm { name: String },
    /// Open (or reuse) a tmux terminal session in the workspace's directory and print the
    /// attach command.
    Terminal { name: String },
}

#[derive(Debug, Subcommand)]
pub(crate) enum HookCommand {
    /// List the project's hook files with last-run status.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Create a hook file from a template; edit it afterwards as a plain file
    /// (`.adi/hooks/<name>` in the project's file browser).
    Create {
        name: String,
        /// The template to start from: init | workspace | blank.
        #[arg(long, default_value = "blank")]
        template: String,
    },
    /// Run a hook detached; output lands in `.adi/hooks/logs/<name>.log`.
    Run {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Print the tail of a hook's most recent run log.
    Log { name: String },
}

/// Dispatch a `projects` subcommand over the adi-core facade, surfacing any store error.
/// Returns `String` errors (like `run_tasks`) so the registry's and adi-hooks' error
/// families print uniformly.
pub(crate) fn run_projects(adi: Adi, command: ProjectsCommand) -> Result<(), String> {
    let store = adi.projects();
    match command {
        ProjectsCommand::List { all, json } => {
            let mut projects = store.list().map_err(|e| e.to_string())?;
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
            name,
            description,
            parent,
            json,
        } => {
            let project = store
                .create(&name, description, parent)
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&project);
            } else {
                println!("Registered project {}.", project.id);
                print_project(&project);
            }
        }
        ProjectsCommand::Show { id, json } => {
            let project = store
                .get(&id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such project: {id}"))?;
            if json {
                print_json(&project);
            } else {
                print_project(&project);
            }
        }
        ProjectsCommand::Archive { id } => {
            let project = store.archive(&id).map_err(|e| e.to_string())?;
            println!("Archived {}.", project.id);
        }
        ProjectsCommand::Unarchive { id } => {
            let project = store.unarchive(&id).map_err(|e| e.to_string())?;
            println!("Restored {}.", project.id);
        }
        ProjectsCommand::Rm { id } => {
            if store.remove(&id).map_err(|e| e.to_string())? {
                println!("Deleted project {id}.");
            } else {
                println!("No such project: {id}.");
            }
        }
        ProjectsCommand::Workspace { project, command } => {
            run_workspace(&store, &project, command)?;
        }
        ProjectsCommand::Hook { project, command } => {
            run_hook(&store, &project, command)?;
        }
    }
    Ok(())
}

/// Resolve a project for the workspace/hook subcommands: its directory plus the
/// `ADI_PROJECT_*` env pairs the hook contract needs.
fn project_scope(
    store: &adi_core::Projects,
    id: &str,
) -> Result<(std::path::PathBuf, Vec<(String, String)>), String> {
    let project = store
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no such project: {id}"))?;
    let dir = store.project_dir(id).map_err(|e| e.to_string())?;
    let env = vec![
        ("ADI_PROJECT_ID".to_string(), project.id.clone()),
        (
            "ADI_PROJECT_NAME".to_string(),
            project.display_name().to_string(),
        ),
    ];
    Ok((dir, env))
}

/// A workspace entry plus its live status, for `--json` output.
#[derive(serde::Serialize)]
struct WorkspaceRow<'a> {
    #[serde(flatten)]
    entry: &'a adi_core::WorkspaceEntry,
    status: &'static str,
}

/// Dispatch a `projects workspace` subcommand over a per-project-dir handle.
fn run_workspace(
    store: &adi_core::Projects,
    project: &str,
    command: WorkspaceCommand,
) -> Result<(), String> {
    let (dir, env) = project_scope(store, project)?;
    let ws = adi_core::Workspaces::new(&dir);
    match command {
        WorkspaceCommand::List { json } => {
            let entries = ws.list().map_err(|e| e.to_string())?;
            if json {
                let rows: Vec<WorkspaceRow> = entries
                    .iter()
                    .map(|e| WorkspaceRow {
                        entry: e,
                        status: ws.status(e).as_str(),
                    })
                    .collect();
                print_json(&rows);
            } else if entries.is_empty() {
                println!("No workspaces. The first `workspace add` runs the init hook.");
            } else {
                for entry in &entries {
                    print_workspace(&ws, entry);
                }
            }
        }
        WorkspaceCommand::Add {
            name,
            path,
            local,
            json,
        } => {
            let explicit = path.map(std::path::PathBuf::from);
            let (entry, run) = ws
                .create(&name, explicit.as_deref(), local, &env)
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&serde_json::json!({
                    "workspace": entry,
                    "status": ws.status(&entry).as_str(),
                    "pid": run.as_ref().map(|r| r.pid),
                    "log": run.as_ref().map(|r| r.log.display().to_string()),
                }));
            } else if let Some(run) = run {
                println!(
                    "Creating workspace {name} via the {} hook (pid {}).",
                    entry.hook.as_deref().unwrap_or("?"),
                    run.pid
                );
                println!("  target: {}", entry.path.display());
                println!("  log: {}", run.log.display());
            } else {
                println!("Linked local workspace {name} at {}.", entry.path.display());
            }
        }
        WorkspaceCommand::Rm { name } => {
            if ws.remove(&name).map_err(|e| e.to_string())? {
                println!("Unregistered workspace {name} (files left on disk).");
            } else {
                println!("No such workspace: {name}.");
            }
        }
        WorkspaceCommand::Terminal { name } => {
            let entry = ws
                .list()
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|w| w.name == name)
                .ok_or_else(|| format!("no such workspace: {name}"))?;
            let session = adi_core::workspace_terminal::open(project, &name, &entry.path)
                .map_err(|e| e.to_string())?;
            println!("Terminal ready in {}.", entry.path.display());
            println!("  {}", session.attach);
        }
    }
    Ok(())
}

/// A hook file plus its last-run status, for `--json` output.
#[derive(serde::Serialize)]
struct HookRow {
    name: String,
    size: u64,
    modified: Option<u64>,
    status: &'static str,
    exit_code: Option<i32>,
    last_run_at: Option<u64>,
}

/// Dispatch a `projects hook` subcommand over a per-project-dir handle.
fn run_hook(store: &adi_core::Projects, project: &str, command: HookCommand) -> Result<(), String> {
    let (dir, mut env) = project_scope(store, project)?;
    let hooks = adi_core::ProjectHooks::new(&dir);
    match command {
        HookCommand::List { json } => {
            let list = hooks.list().map_err(|e| e.to_string())?;
            if json {
                let rows: Vec<HookRow> = list
                    .into_iter()
                    .map(|h| {
                        let status = hooks.status(&h.name);
                        HookRow {
                            last_run_at: hooks.last_run(&h.name),
                            status: status.as_str(),
                            exit_code: status.exit_code(),
                            name: h.name,
                            size: h.size,
                            modified: h.modified,
                        }
                    })
                    .collect();
                print_json(&rows);
            } else if list.is_empty() {
                println!("No hooks. Create one with `projects hook {project} create init --template init`.");
            } else {
                for hook in &list {
                    let status = hooks.status(&hook.name);
                    println!("{} [{}] — {} B", hook.name, status.as_str(), hook.size);
                    if let Some(ran) = hooks.last_run(&hook.name) {
                        println!("  last run: {ran} (unix)");
                    }
                }
            }
        }
        HookCommand::Create { name, template } => {
            let body = adi_core::hook_template(&template)
                .ok_or_else(|| format!("unknown template {template:?} (init | workspace | blank)"))?;
            hooks.create(&name, body).map_err(|e| e.to_string())?;
            let path = hooks.hook_path(&name).map_err(|e| e.to_string())?;
            println!("Created hook {name} at {} — edit it there.", path.display());
        }
        HookCommand::Run { name, json } => {
            if adi_core::is_lifecycle(&name) {
                return Err(format!(
                    "the {name} hook runs when a workspace is created — use `projects workspace {project} add <name>`"
                ));
            }
            env.push(("ADI_PROJECT_DIR".to_string(), dir.display().to_string()));
            let run = hooks.run(&name, &env, &dir).map_err(|e| e.to_string())?;
            if json {
                print_json(&serde_json::json!({
                    "pid": run.pid,
                    "log": run.log.display().to_string(),
                }));
            } else {
                println!("Running hook {name} (pid {}).", run.pid);
                println!("  log: {}", run.log.display());
            }
        }
        HookCommand::Log { name } => match hooks.read_log(&name) {
            Some(log) => print!("{log}"),
            None => println!("Hook {name} never ran."),
        },
    }
    Ok(())
}

/// Print a workspace as a human line plus its metadata, mirroring `print_project`.
fn print_workspace(ws: &adi_core::Workspaces, entry: &adi_core::WorkspaceEntry) {
    println!(
        "{} — {} [{}]",
        entry.name,
        entry.path.display(),
        ws.status(entry).as_str()
    );
    let mut meta = vec![format!("kind: {}", entry.kind.as_str())];
    if let Some(hook) = &entry.hook {
        meta.push(format!("hook: {hook}"));
    }
    if let Some(pid) = entry.pid {
        meta.push(format!("pid: {pid}"));
    }
    println!("  {}", meta.join(" · "));
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
