//! The `tasks` command group: the task-tree subcommand surface and its dispatch over
//! the shared task store.

use adi_core::{Adi, TaskPatch, TaskView};
use clap::Subcommand;

use crate::format::{parse_effective_status_opt, parse_task_status_opt, print_json};

#[derive(Debug, Subcommand)]
pub(crate) enum TasksCommand {
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

/// Dispatch a `tasks` subcommand over the shared task store.
pub(crate) fn run_tasks(adi: Adi, command: TasksCommand) -> Result<(), String> {
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
            tasks.sort_by(|a, b| a.order(b));
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

/// Print a task in the compact human CLI format.
fn print_task(task: &TaskView) {
    println!(
        "{} — {} [{}]",
        task.task.id,
        task.task.title,
        task.effective.as_str()
    );
    let mut meta = vec![format!("status: {}", task.task.status.as_str())];
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
