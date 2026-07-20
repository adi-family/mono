//! The `tools` command group: register, edit, and run *tools* — small CLIs an agent invokes.
//! A thin argv adapter over [`adi_core::Tools`]; `tools run <id>` is what the generated
//! `.bin/<name>` shims exec, so it inherits the caller's stdio and forwards the exit code.

use std::path::PathBuf;

use adi_core::{Adi, Tool};
use clap::Subcommand;

use crate::format::print_json;

#[derive(Debug, Subcommand)]
pub(crate) enum ToolsCommand {
    /// List registered tools (active only unless `--all`).
    List {
        /// Only tools filed under this project id.
        #[arg(long)]
        project: Option<String>,
        /// Include archived tools.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Create a new owned tool: writes a script into the store (`tools/<id>/script.<ext>`) and a
    /// `.bin/<name>` shim. The script starts from a runtime template unless `--from` seeds it.
    Add {
        /// The display name; also the basis for the `.bin/<name>` shim.
        name: String,
        /// The script language: `sh` or `ts`.
        #[arg(long, default_value = "sh")]
        runtime: String,
        #[arg(long)]
        description: Option<String>,
        /// File the tool under this project id (a project-scoped tool); omit for a global tool.
        #[arg(long)]
        project: Option<String>,
        /// Seed the new script from this file instead of the runtime template.
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Reuse an existing script by linking it in place: writes a manifest referencing `path` (the
    /// file is never copied) plus a `.bin/<name>` shim. Runtime is inferred from the extension.
    Link {
        /// The absolute or relative path to an existing sh/ts file.
        path: String,
        /// The display name; defaults to the file's stem.
        #[arg(long)]
        name: Option<String>,
        /// Override the inferred runtime (`sh` | `ts`).
        #[arg(long)]
        runtime: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show one tool's manifest.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Print a tool's script to stdout.
    Cat { id: String },
    /// Print the resolved path of a tool's script (its owned file, or the linked target).
    Path { id: String },
    /// Run a tool, forwarding every following argument to it. Inherits this terminal's stdio and
    /// exits with the tool's exit code — this is what the `.bin/<name>` shims exec.
    Run {
        id: String,
        /// Arguments passed through to the tool verbatim (put them after `--`).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Archive a tool (soft delete; drops its `.bin` shim). Reversible with `unarchive`.
    Archive { id: String },
    /// Restore an archived tool (re-creates its `.bin` shim).
    Unarchive { id: String },
    /// Permanently delete a tool. A linked tool's target file is never touched.
    Rm { id: String },
    /// Regenerate the global `.bin` shims from the current manifests.
    SyncBin {
        #[arg(long)]
        json: bool,
    },
    /// Ensure the built-in system tools (adi-tasks, adi-projects, …) exist, then rebuild the `.bin`.
    Seed,
}

/// Dispatch a `tools` subcommand over the adi-core facade, surfacing any store error as a
/// `String` (like `run_tasks`) so the CLI prints them uniformly.
pub(crate) fn run_tools(adi: Adi, command: ToolsCommand) -> Result<(), String> {
    let store = adi.tools();
    match command {
        ToolsCommand::List {
            project,
            all,
            json,
        } => {
            let mut tools = store.list().map_err(|e| e.to_string())?;
            if !all {
                tools.retain(|t| !t.is_archived());
            }
            if let Some(pid) = project.as_deref() {
                tools.retain(|t| t.manifest.project.as_deref() == Some(pid));
            }
            if json {
                print_json(&tools);
            } else if tools.is_empty() {
                println!("No tools registered.");
            } else {
                for tool in &tools {
                    print_tool(&tool);
                }
                println!("\nRun a tool with: {} tools run <id> [args…]", adi_core::BIN_NAME);
                println!("Or put {} on your PATH.", store.bin_dir().display());
            }
        }
        ToolsCommand::Add {
            name,
            runtime,
            description,
            project,
            from,
            json,
        } => {
            let content = match from {
                Some(path) => Some(
                    std::fs::read_to_string(&path)
                        .map_err(|e| format!("couldn't read --from {path}: {e}"))?,
                ),
                None => None,
            };
            let tool = store
                .create_file(&name, description, &runtime, project, content)
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&tool);
            } else {
                println!("Created tool {} ({}).", tool.display_name(), tool.id);
                print_tool(&tool);
            }
        }
        ToolsCommand::Link {
            path,
            name,
            runtime,
            description,
            project,
            json,
        } => {
            let tool = store
                .link(&path, name, runtime, description, project)
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&tool);
            } else {
                println!("Linked tool {} ({}).", tool.display_name(), tool.id);
                print_tool(&tool);
            }
        }
        ToolsCommand::Show { id, json } => {
            let tool = store
                .get(&id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such tool: {id}"))?;
            if json {
                print_json(&tool);
            } else {
                print_tool(&tool);
            }
        }
        ToolsCommand::Cat { id } => {
            let body = store.read_script(&id).map_err(|e| e.to_string())?;
            print!("{body}");
        }
        ToolsCommand::Path { id } => {
            let path = store.script_path(&id).map_err(|e| e.to_string())?;
            println!("{}", path.display());
        }
        ToolsCommand::Run { id, args } => {
            // Resolve the working dir: a project-scoped tool runs in its project directory, a
            // global tool in the store root (the store's default).
            let cwd = working_dir(&adi, &store, &id);
            let mut cmd = store
                .command(&id, &args, cwd.as_deref())
                .map_err(|e| e.to_string())?;
            let status = cmd
                .status()
                .map_err(|e| format!("couldn't run tool {id}: {e}"))?;
            std::process::exit(status.code().unwrap_or(1));
        }
        ToolsCommand::Archive { id } => {
            let tool = store.archive(&id).map_err(|e| e.to_string())?;
            println!("Archived {}.", tool.id);
        }
        ToolsCommand::Unarchive { id } => {
            let tool = store.unarchive(&id).map_err(|e| e.to_string())?;
            println!("Restored {}.", tool.id);
        }
        ToolsCommand::Rm { id } => {
            if store.remove(&id).map_err(|e| e.to_string())? {
                println!("Deleted tool {id}.");
            } else {
                println!("No such tool: {id}.");
            }
        }
        ToolsCommand::SyncBin { json } => {
            let written = store.sync_bin().map_err(|e| e.to_string())?;
            if json {
                print_json(&written);
            } else if written.is_empty() {
                println!("No active tools — {} is empty.", store.bin_dir().display());
            } else {
                println!("Regenerated {} shims in {}:", written.len(), store.bin_dir().display());
                for (name, id) in &written {
                    println!("  {name} -> {id}");
                }
            }
        }
        ToolsCommand::Seed => {
            let created = store.seed_system().map_err(|e| e.to_string())?;
            let written = store.sync_bin().map_err(|e| e.to_string())?;
            if created {
                println!("Seeded the built-in system tools.");
            } else {
                println!("System tools already present.");
            }
            println!("Global .bin now has {} shims in {}.", written.len(), store.bin_dir().display());
        }
    }
    Ok(())
}

/// Resolve where a tool runs: a project-scoped tool in its project directory (when that project is
/// registered), else `None` so the store falls back to its root.
fn working_dir(adi: &Adi, store: &adi_core::Tools, id: &str) -> Option<PathBuf> {
    let tool = store.get(id).ok().flatten()?;
    let project = tool.manifest.project?;
    adi.projects().project_dir(&project).ok()
}

/// Print a tool as a human line plus its metadata, mirroring `print_project`.
fn print_tool(tool: &Tool) {
    let state = if tool.is_archived() { "archived" } else { "active" };
    let source = if tool.is_system() {
        "system"
    } else if tool.is_linked() {
        "linked"
    } else {
        "owned"
    };
    println!(
        "{} — {} [{state}] {} · {source} · .bin/{}",
        tool.id,
        tool.display_name(),
        tool.runtime(),
        tool.bin_name(),
    );
    if let Some(description) = &tool.manifest.description {
        println!("  {description}");
    }
    if let Some(path) = &tool.manifest.linked_path {
        println!("  path: {path}");
    }
    if let Some(project) = &tool.manifest.project {
        println!("  project: {project}");
    }
}
