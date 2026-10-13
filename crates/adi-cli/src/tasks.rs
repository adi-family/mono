//! The `tasks` command group: the task-tree subcommand surface and its dispatch over
//! the shared task store.

use std::path::PathBuf;

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
        /// Where this task's work happens — the directory a run picking it up starts in.
        /// A subtask inherits it from its parent unless it states its own.
        #[arg(long)]
        cwd: Option<String>,
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
        /// Where this task's work happens; pass an empty string to clear it.
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Mark a task done. If it has a required output shape, `--result` (or `--result-file`)
    /// must satisfy it or the call is refused with the exact path of the mismatch.
    Complete {
        id: String,
        /// The result, as a JSON literal.
        #[arg(long)]
        result: Option<String>,
        /// The result, read from a file of JSON.
        #[arg(long)]
        result_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Mark a task done. If it has a required output shape, `--result` (or `--result-file`)
    /// must satisfy it or the call is refused with the exact path of the mismatch.
    Done {
        id: String,
        /// The result, as a JSON literal.
        #[arg(long)]
        result: Option<String>,
        /// The result, read from a file of JSON.
        #[arg(long)]
        result_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Give up on a task without finishing it — a fact recorded for whatever dispatches it,
    /// leaving the task's status untouched (reopening it clears the reason).
    GiveUp {
        id: String,
        #[arg(long)]
        reason: String,
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
            cwd,
            json,
        } => {
            let task = store
                .create(title, details, project, tag, parent, cwd)
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
                print_task_show(&task);
            }
        }
        TasksCommand::Edit {
            id,
            title,
            details,
            tag,
            assignee,
            parent,
            cwd,
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
                        cwd,
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
        TasksCommand::Complete {
            id,
            result,
            result_file,
            json,
        }
        | TasksCommand::Done {
            id,
            result,
            result_file,
            json,
        } => {
            let result = parse_result(result, result_file)?;
            let task = store.complete(&id, result).map_err(|e| e.to_string())?;
            if json {
                print_json(&task);
            } else {
                println!("Completed task {}.", task.task.id);
                print_task(&task);
            }
        }
        TasksCommand::GiveUp { id, reason, json } => {
            let task = store.give_up(&id, &reason).map_err(|e| e.to_string())?;
            if json {
                print_json(&task);
            } else {
                println!("Gave up on task {}.", task.task.id);
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

/// Parse `--result`/`--result-file` into the JSON value to pass to `complete`, if either was
/// given. Giving both is refused rather than silently preferring one.
fn parse_result(
    result: Option<String>,
    result_file: Option<PathBuf>,
) -> Result<Option<serde_json::Value>, String> {
    match (result, result_file) {
        (Some(_), Some(_)) => Err("pass either --result or --result-file, not both".to_string()),
        (Some(s), None) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| format!("--result is not valid JSON: {e}")),
        (None, Some(path)) => {
            let bytes = std::fs::read(&path)
                .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
        }
        (None, None) => Ok(None),
    }
}

/// Print a task in the compact human CLI format.
fn print_task(task: &TaskView) {
    println!(
        "{} — {} [{}]",
        task.task.id,
        task.task.title,
        task.effective.as_str()
    );
    println!("  {}", task_meta_line(task));
    if let Some(details) = &task.task.details {
        println!("  {details}");
    }
}

/// The one-line `key: value · key: value` summary shared by [`print_task`] and
/// [`print_task_show`].
fn task_meta_line(task: &TaskView) -> String {
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
    if let Some(cwd) = &task.task.cwd {
        meta.push(format!("cwd: {cwd}"));
    }
    if let Some(step_id) = task.task.step_id() {
        meta.push(format!("step: {step_id}"));
    }
    if task.children_total > 0 {
        meta.push(format!(
            "subtasks: {}/{} open",
            task.children_open, task.children_total
        ));
    }
    if let Some(reason) = &task.task.give_up_reason {
        meta.push(format!("gave up: {reason}"));
    }
    meta.join(" · ")
}

/// Print a task the way `tasks show` does: the compact summary, then its instructions, its
/// input, the output shape a result must satisfy, and the exact commands to finish or give up.
fn print_task_show(task: &TaskView) {
    print_task(task);
    println!();
    println!("Instructions:");
    match &task.task.details {
        Some(details) => println!("  {details}"),
        None => println!("  (none)"),
    }
    if let Some(input) = &task.task.input {
        println!();
        println!("Input:");
        print_indented_json(input);
    }
    if let Some(schema) = &task.task.output_schema {
        println!();
        println!("Required output shape:");
        print_indented_json(schema);
    }
    println!();
    println!("To finish:");
    if task.task.output_schema.is_some() {
        println!(
            "  adi-mono tasks complete {} --result '<json matching the shape above>'",
            task.task.id
        );
    } else {
        println!("  adi-mono tasks complete {}", task.task.id);
    }
    println!("To give up:");
    println!("  adi-mono tasks give-up {} --reason \"<why>\"", task.task.id);
}

/// Pretty-print `value` indented two spaces, for the blocks `print_task_show` shows under
/// "Input:" and "Required output shape:".
fn print_indented_json(value: &serde_json::Value) {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    for line in pretty.lines() {
        println!("  {line}");
    }
}
