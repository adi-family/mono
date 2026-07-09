//! The `tasks` feature: the MCP `tasks_*` tools and the `adi-task` CLI, both thin frontends over
//! the shared [`adi_tasks`] store (`~/.adi/mono/mcp/tasks.json`). The store owns all task-tree
//! logic — the stored `open`/`done`/`archived` status, the computed `ready`/`blocked`/`done`/
//! `archived` effective status, and every tree edit; this module only adapts it to MCP tool
//! calls and to shell commands, so tasks created either way share one on-disk tree.

use adi_tasks::{EffectiveStatus, TaskPatch, TaskStatus, TaskView, Tasks};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::server::{AdiMcp, internal, json_result, text_result};

/// Map a store [`adi_tasks::Error`] to a fitting MCP error: the client errors become
/// `invalid_params`, a wrapped store failure becomes an `internal_error`.
// Consumed by value because it is used as a `.map_err(task_err)` adapter, which hands the error
// over by value.
#[allow(clippy::needless_pass_by_value)]
fn task_err(e: adi_tasks::Error) -> McpError {
    match e {
        // A wrapped store failure is internal; every other variant is a client error whose
        // message is its `Display`.
        adi_tasks::Error::Store(inner) => internal("task store error", inner),
        other => McpError::invalid_params(other.to_string(), None),
    }
}

// ---- MCP tools -------------------------------------------------------------------------

/// Arguments for `tasks_create`.
#[derive(Debug, Deserialize, JsonSchema)]
struct CreateTaskArgs {
    /// Short one-line title for the task (required).
    title: String,
    /// Optional longer details or notes.
    #[serde(default)]
    details: Option<String>,
    /// Optional adi project id to associate the task with.
    #[serde(default)]
    project: Option<String>,
    /// Optional free-form tag / label.
    #[serde(default)]
    tag: Option<String>,
    /// Optional parent task id; if given it must already exist (creates a subtask).
    #[serde(default)]
    parent: Option<String>,
}

/// Arguments for `tasks_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ListTasksArgs {
    /// Only return tasks associated with this project id.
    #[serde(default)]
    project: Option<String>,
    /// Only return tasks carrying this tag.
    #[serde(default)]
    tag: Option<String>,
    /// Only return tasks in this stored status (`open`, `done`, `archived`).
    #[serde(default)]
    status: Option<TaskStatus>,
    /// Only return tasks with this computed status (`ready`, `blocked`, `done`, `archived`).
    #[serde(default)]
    effective: Option<EffectiveStatus>,
}

/// Arguments for `tasks_get`, `tasks_complete`, `tasks_reopen`, and `tasks_delete`.
#[derive(Debug, Deserialize, JsonSchema)]
struct TaskIdArgs {
    /// The task id (e.g. `t1`).
    id: String,
}

/// Arguments for `tasks_update`. Only the fields you pass change; omit a field to leave it.
#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateTaskArgs {
    /// The task id to update (e.g. `t1`).
    id: String,
    /// New title (unchanged if omitted; must not be blank if given).
    #[serde(default)]
    title: Option<String>,
    /// New details; pass an empty string to clear (unchanged if omitted).
    #[serde(default)]
    details: Option<String>,
    /// New tag; pass an empty string to clear (unchanged if omitted).
    #[serde(default)]
    tag: Option<String>,
    /// New assignee; pass an empty string to clear (unchanged if omitted).
    #[serde(default)]
    assignee: Option<String>,
    /// New parent id; pass an empty string to detach to root. Must exist and must not create a
    /// cycle. Unchanged if omitted.
    #[serde(default)]
    parent: Option<String>,
}

/// Arguments for `tasks_archive`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArchiveTaskArgs {
    /// The task id to archive (e.g. `t1`).
    id: String,
    /// Also archive every still-open descendant (default: `true`).
    #[serde(default = "default_true")]
    cascade: bool,
}

/// The serde default for [`ArchiveTaskArgs::cascade`].
fn default_true() -> bool {
    true
}

/// The success shape of `tasks_complete` when the completed task still has open subtasks.
#[derive(Debug, Serialize)]
struct CompleteWithWarning {
    task: TaskView,
    warning: String,
}

#[tool_router(router = tasks_router, vis = "pub")]
impl AdiMcp {
    #[tool(description = "Create a task (stored status open) and return its view; a given parent must already exist")]
    async fn tasks_create(
        &self,
        Parameters(args): Parameters<CreateTaskArgs>,
    ) -> Result<CallToolResult, McpError> {
        let title = args.title.trim().to_string();
        if title.is_empty() {
            return Err(McpError::invalid_params("title must not be empty", None));
        }
        let view = Tasks::open()
            .create(title, args.details, args.project, args.tag, args.parent)
            .map_err(task_err)?;
        json_result(&view)
    }

    #[tool(
        description = "List task views (with computed effective status), filtered by any of project, tag, stored status, or effective status"
    )]
    async fn tasks_list(
        &self,
        Parameters(args): Parameters<ListTasksArgs>,
    ) -> Result<CallToolResult, McpError> {
        let views = Tasks::open()
            .list(args.project, args.tag, args.status, args.effective)
            .map_err(task_err)?;
        json_result(&views)
    }

    #[tool(description = "Get one task view by id, including its computed effective status")]
    async fn tasks_get(
        &self,
        Parameters(args): Parameters<TaskIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let view = Tasks::open().get(&args.id).map_err(task_err)?;
        json_result(&view)
    }

    #[tool(
        description = "Edit a task's title, details, tag, assignee, or parent (not its status); only provided fields change. Empty string clears an optional field; reparenting must not create a cycle"
    )]
    async fn tasks_update(
        &self,
        Parameters(args): Parameters<UpdateTaskArgs>,
    ) -> Result<CallToolResult, McpError> {
        let title = args.title.map(|t| t.trim().to_string());
        if matches!(&title, Some(t) if t.is_empty()) {
            return Err(McpError::invalid_params("title must not be empty", None));
        }
        let patch = TaskPatch {
            title,
            details: args.details,
            tag: args.tag,
            assignee: args.assignee,
            parent: args.parent,
        };
        let view = Tasks::open()
            .update(&args.id, patch)
            .map_err(task_err)?;
        json_result(&view)
    }

    #[tool(
        description = "Mark a task done (idempotent). Completing a task that still has open subtasks succeeds but returns a warning. An archived task must be reopened first"
    )]
    async fn tasks_complete(
        &self,
        Parameters(args): Parameters<TaskIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let view = Tasks::open().complete(&args.id).map_err(task_err)?;
        if view.children_open >= 1 {
            let warning = format!(
                "task {} completed, but it still has {} open direct subtask(s)",
                view.task.id, view.children_open
            );
            return json_result(&CompleteWithWarning {
                task: view,
                warning,
            });
        }
        json_result(&view)
    }

    #[tool(
        description = "Archive a task; by default (cascade) also archives every still-open descendant"
    )]
    async fn tasks_archive(
        &self,
        Parameters(args): Parameters<ArchiveTaskArgs>,
    ) -> Result<CallToolResult, McpError> {
        let view = Tasks::open()
            .archive(&args.id, args.cascade)
            .map_err(task_err)?;
        json_result(&view)
    }

    #[tool(description = "Reopen a done or archived task, setting its stored status back to open")]
    async fn tasks_reopen(
        &self,
        Parameters(args): Parameters<TaskIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let view = Tasks::open().reopen(&args.id).map_err(task_err)?;
        json_result(&view)
    }

    #[tool(
        description = "Delete a task permanently; its direct children are reparented to the deleted task's parent"
    )]
    async fn tasks_delete(
        &self,
        Parameters(args): Parameters<TaskIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        Tasks::open().delete(&args.id).map_err(task_err)?;
        Ok(text_result(format!("deleted task {}", args.id)))
    }
}

/// `adi-task` — a tiny command-line frontend over the *same* task store the MCP `tasks` feature
/// uses (`~/.adi/mono/mcp/tasks.json`). Shipped as its own binary so the task tree can be created
/// and driven by hand or from scripts, not only over MCP. A child module so it can reuse the
/// store types directly; only [`cli::run`] is re-exported (as `adi_mcp::run_tasks_cli`).
pub(crate) mod cli {
    use clap::{Parser, Subcommand};

    use super::{EffectiveStatus, TaskPatch, TaskStatus, TaskView, Tasks};

    /// Manage the adi task tree from the shell (shares state with the adi-mcp `tasks` tools).
    #[derive(Debug, Parser)]
    #[command(name = "adi-task", version, about)]
    struct Cli {
        #[command(subcommand)]
        command: Command,
    }

    #[derive(Debug, Subcommand)]
    enum Command {
        /// Create a task (stored status `open`).
        Add {
            /// Short one-line title.
            title: String,
            /// Longer details / notes.
            #[arg(long)]
            details: Option<String>,
            /// Associated adi project id.
            #[arg(long)]
            project: Option<String>,
            /// Free-form tag / label.
            #[arg(long)]
            tag: Option<String>,
            /// Parent task id — makes this a subtask (the parent must already exist).
            #[arg(long)]
            parent: Option<String>,
        },
        /// List tasks with their computed effective status.
        List {
            /// Only tasks in this project.
            #[arg(long)]
            project: Option<String>,
            /// Only tasks carrying this tag.
            #[arg(long)]
            tag: Option<String>,
            /// Filter by stored status: `open` | `done` | `archived`.
            #[arg(long)]
            status: Option<String>,
            /// Filter by effective status: `ready` | `blocked` | `done` | `archived`.
            #[arg(long)]
            effective: Option<String>,
            /// Emit JSON instead of the text table.
            #[arg(long)]
            json: bool,
        },
        /// Show one task in full.
        Show {
            /// The task id (e.g. `t1`).
            id: String,
            /// Emit JSON.
            #[arg(long)]
            json: bool,
        },
        /// Mark a task done.
        Done {
            /// The task id.
            id: String,
        },
        /// Archive a task (and, by default, its still-open descendants).
        Archive {
            /// The task id.
            id: String,
            /// Archive only this task, not its open descendants.
            #[arg(long)]
            no_cascade: bool,
        },
        /// Reopen a done/archived task back to `open`.
        Reopen {
            /// The task id.
            id: String,
        },
        /// Edit a task's fields (not its status); empty string clears an optional field.
        Update {
            /// The task id.
            id: String,
            #[arg(long)]
            title: Option<String>,
            #[arg(long)]
            details: Option<String>,
            #[arg(long)]
            tag: Option<String>,
            #[arg(long)]
            assignee: Option<String>,
            /// New parent id; pass an empty string to detach to root.
            #[arg(long)]
            parent: Option<String>,
        },
        /// Delete a task permanently (its children reparent to its parent).
        Rm {
            /// The task id.
            id: String,
        },
    }

    /// Entry point for the `adi-task` binary.
    ///
    /// # Errors
    /// Propagates store failures — I/O, an unknown id, a missing parent, or a would-be cycle — as
    /// `anyhow` errors, plus a parse error for an invalid `--status`/`--effective` value.
    pub fn run() -> anyhow::Result<()> {
        let cli = Cli::parse();
        let store = Tasks::open();
        match cli.command {
            Command::Add {
                title,
                details,
                project,
                tag,
                parent,
            } => print_line(&store.create(title, details, project, tag, parent)?),
            Command::List {
                project,
                tag,
                status,
                effective,
                json,
            } => {
                let status = status.as_deref().map(parse_status).transpose()?;
                let effective = effective.as_deref().map(parse_effective).transpose()?;
                let mut views = store.list(project, tag, status, effective)?;
                views.sort_by_key(|v| task_num(&v.task.id));
                if json {
                    println!("{}", serde_json::to_string_pretty(&views)?);
                } else if views.is_empty() {
                    println!("(no tasks)");
                } else {
                    for v in &views {
                        print_line(v);
                    }
                }
            }
            Command::Show { id, json } => {
                let view = store.get(&id)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&view)?);
                } else {
                    print_show(&view);
                }
            }
            Command::Done { id } => {
                let view = store.complete(&id)?;
                print_line(&view);
                if view.children_open >= 1 {
                    eprintln!(
                        "warning: completed with {} still-open subtask(s)",
                        view.children_open
                    );
                }
            }
            Command::Archive { id, no_cascade } => print_line(&store.archive(&id, !no_cascade)?),
            Command::Reopen { id } => print_line(&store.reopen(&id)?),
            Command::Update {
                id,
                title,
                details,
                tag,
                assignee,
                parent,
            } => {
                let patch = TaskPatch {
                    title,
                    details,
                    tag,
                    assignee,
                    parent,
                };
                print_line(&store.update(&id, patch)?);
            }
            Command::Rm { id } => {
                store.delete(&id)?;
                println!("deleted {id}");
            }
        }
        Ok(())
    }

    /// Parse a stored-status filter.
    fn parse_status(s: &str) -> anyhow::Result<TaskStatus> {
        match s.trim().to_ascii_lowercase().as_str() {
            "open" => Ok(TaskStatus::Open),
            "done" => Ok(TaskStatus::Done),
            "archived" => Ok(TaskStatus::Archived),
            other => anyhow::bail!("invalid --status {other:?}; use open|done|archived"),
        }
    }

    /// Parse an effective-status filter.
    fn parse_effective(s: &str) -> anyhow::Result<EffectiveStatus> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ready" => Ok(EffectiveStatus::Ready),
            "blocked" => Ok(EffectiveStatus::Blocked),
            "done" => Ok(EffectiveStatus::Done),
            "archived" => Ok(EffectiveStatus::Archived),
            other => {
                anyhow::bail!("invalid --effective {other:?}; use ready|blocked|done|archived")
            }
        }
    }

    /// The numeric part of a `t<N>` id, for stable ordering (0 if it doesn't parse).
    fn task_num(id: &str) -> u64 {
        id.strip_prefix('t').and_then(|n| n.parse().ok()).unwrap_or(0)
    }

    /// The display label for a stored status.
    fn stored_label(s: TaskStatus) -> &'static str {
        match s {
            TaskStatus::Open => "open",
            TaskStatus::Done => "done",
            TaskStatus::Archived => "archived",
        }
    }

    /// The display label for a computed effective status.
    fn effective_label(e: EffectiveStatus) -> &'static str {
        match e {
            EffectiveStatus::Ready => "ready",
            EffectiveStatus::Blocked => "blocked",
            EffectiveStatus::Done => "done",
            EffectiveStatus::Archived => "archived",
        }
    }

    /// One compact line: `t1   [blocked ]  Title   (1/2 open)  @project  #tag`.
    fn print_line(v: &TaskView) {
        let children = if v.children_total > 0 {
            format!("  ({}/{} open)", v.children_open, v.children_total)
        } else {
            String::new()
        };
        let project = v.task.project.as_deref().map_or(String::new(), |p| format!("  @{p}"));
        let tag = v.task.tag.as_deref().map_or(String::new(), |t| format!("  #{t}"));
        println!(
            "{:<5} [{:<8}] {}{children}{project}{tag}",
            v.task.id,
            effective_label(v.effective),
            v.task.title
        );
    }

    /// The full task, one field per line.
    fn print_show(v: &TaskView) {
        println!("id:        {}", v.task.id);
        println!("title:     {}", v.task.title);
        println!("stored:    {}", stored_label(v.task.status));
        println!("effective: {}", effective_label(v.effective));
        if let Some(p) = &v.task.parent {
            println!("parent:    {p}");
        }
        println!("children:  {} ({} open)", v.children_total, v.children_open);
        if let Some(p) = &v.task.project {
            println!("project:   {p}");
        }
        if let Some(t) = &v.task.tag {
            println!("tag:       {t}");
        }
        if let Some(a) = &v.task.assignee {
            println!("assignee:  {a}");
        }
        if let Some(d) = &v.task.details {
            println!("details:   {d}");
        }
    }
}
