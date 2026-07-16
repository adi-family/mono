//! Bundled task tools, rewired from the old `adi.workforce.capability.tasks`
//! plugin onto this repo's [`adi_tasks`] store. The old plugin's vault
//! machinery (per-vault task databases) does not exist here — adi-tasks is
//! one store with project scoping, so tools take an optional `project`
//! binding in their config instead of `vaults`.
//!
//! Registered tool ids keep the old SDK surface where the semantics map:
//! `TaskCreate`, `TaskGet`, `TaskList`, `TaskUpdate`, `TaskResolve`.
//! (`TaskComment`/`TaskHistory` had no adi-tasks counterpart and are not
//! ported.)

use std::sync::Arc;

use adi_tasks::{TaskPatch, TaskStatus, Tasks};

use crate::config_value::ConfigValue;
use crate::loop_run_context::LoopRunContext;
use crate::plugin::PluginError;
use crate::tool_def::{Tool, ToolCallError};

/// Shared system-prompt fragment for the task tool family — deduped by the
/// engine so it appears once no matter how many task tools a loop carries.
const TASKS_GUIDANCE: &str = "### Task tools\n\
Tasks form a tree: a task with open children is blocked, one without is ready.\n\
Use task_create with parent to decompose work; resolve tasks bottom-up.";

/// The project a tool instance is bound to, from its config (`{ project }`).
/// `None` means unscoped: the LLM may pass/omit `project` per call.
fn bound_project(config: &ConfigValue) -> Option<String> {
    config
        .get("project")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

fn arg_str(args: &ConfigValue, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn view_json(view: &adi_tasks::TaskView) -> String {
    serde_json::to_string(view).unwrap_or_else(|e| format!("{{\"error\":\"serialize: {e}\"}}"))
}

fn parse_json(raw: &str) -> Result<ConfigValue, ToolCallError> {
    ConfigValue::from_json(raw).map_err(|e| ToolCallError::Internal(format!("invalid JSON: {e}")))
}

// ── TaskCreate ──

pub struct TaskCreateTool {
    project: Option<String>,
}

impl TaskCreateTool {
    /// Factory for registry id `TaskCreate`.
    ///
    /// # Errors
    /// Never fails; the signature matches [`crate::core::ToolCreateFn`].
    pub fn create(config: ConfigValue) -> Result<Arc<dyn Tool>, PluginError> {
        Ok(Arc::new(Self {
            project: bound_project(&config),
        }))
    }
}

impl Tool for TaskCreateTool {
    fn name(&self) -> String {
        "task_create".to_string()
    }
    fn description(&self) -> String {
        "Create a task. Use parent_id to decompose an existing task into subtasks.".to_string()
    }
    fn system_prompt(&self) -> Option<String> {
        Some(TASKS_GUIDANCE.to_string())
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"title":{"type":"string"},"details":{"type":"string"},"tag":{"type":"string","description":"Free-form tag; a tag equal to an agent name assigns the task to that agent"},"parent_id":{"type":"string"},"project":{"type":"string","description":"Project id; omit to use the tool's configured project"}},"required":["title"]}"#.to_string()
    }
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        let args = parse_json(raw)?;
        if arg_str(&args, "title").is_none() {
            return Err(ToolCallError::BadRequest("missing 'title'".to_string()));
        }
        Ok(args)
    }
    fn execute(&self, _ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError> {
        let title = arg_str(&args, "title")
            .ok_or_else(|| PluginError::new("task_create: missing 'title'"))?;
        let project = arg_str(&args, "project").or_else(|| self.project.clone());
        let view = Tasks::open()
            .create(
                title,
                arg_str(&args, "details"),
                project,
                arg_str(&args, "tag"),
                arg_str(&args, "parent_id"),
            )
            .map_err(|e| PluginError::new(format!("task_create: {e}")))?;
        Ok(view_json(&view))
    }
}

// ── TaskGet ──

pub struct TaskGetTool;

impl TaskGetTool {
    /// Factory for registry id `TaskGet`.
    ///
    /// # Errors
    /// Never fails; the signature matches [`crate::core::ToolCreateFn`].
    pub fn create(_config: ConfigValue) -> Result<Arc<dyn Tool>, PluginError> {
        Ok(Arc::new(Self))
    }
}

impl Tool for TaskGetTool {
    fn name(&self) -> String {
        "task_get".to_string()
    }
    fn description(&self) -> String {
        "Get one task by id, including its computed ready/blocked status.".to_string()
    }
    fn system_prompt(&self) -> Option<String> {
        Some(TASKS_GUIDANCE.to_string())
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#.to_string()
    }
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        let args = parse_json(raw)?;
        if arg_str(&args, "id").is_none() {
            return Err(ToolCallError::BadRequest("missing 'id'".to_string()));
        }
        Ok(args)
    }
    fn execute(&self, _ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError> {
        let id = arg_str(&args, "id").ok_or_else(|| PluginError::new("task_get: missing 'id'"))?;
        let view = Tasks::open()
            .get(&id)
            .map_err(|e| PluginError::new(format!("task_get: {e}")))?;
        Ok(view_json(&view))
    }
}

// ── TaskList ──

pub struct TaskListTool {
    project: Option<String>,
}

impl TaskListTool {
    /// Factory for registry id `TaskList`.
    ///
    /// # Errors
    /// Never fails; the signature matches [`crate::core::ToolCreateFn`].
    pub fn create(config: ConfigValue) -> Result<Arc<dyn Tool>, PluginError> {
        Ok(Arc::new(Self {
            project: bound_project(&config),
        }))
    }
}

impl Tool for TaskListTool {
    fn name(&self) -> String {
        "task_list".to_string()
    }
    fn description(&self) -> String {
        "List tasks, optionally filtered by project, tag, or status (open|done|archived)."
            .to_string()
    }
    fn system_prompt(&self) -> Option<String> {
        Some(TASKS_GUIDANCE.to_string())
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"project":{"type":"string"},"tag":{"type":"string"},"status":{"type":"string","enum":["open","done","archived"]}}}"#.to_string()
    }
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        parse_json(raw)
    }
    fn execute(&self, _ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError> {
        let status = match arg_str(&args, "status").as_deref() {
            Some("open") => Some(TaskStatus::Open),
            Some("done") => Some(TaskStatus::Done),
            Some("archived") => Some(TaskStatus::Archived),
            Some(other) => {
                return Err(PluginError::new(format!(
                    "task_list: unknown status '{other}'"
                )));
            }
            None => None,
        };
        let project = arg_str(&args, "project").or_else(|| self.project.clone());
        let views = Tasks::open()
            .list(project, arg_str(&args, "tag"), status, None)
            .map_err(|e| PluginError::new(format!("task_list: {e}")))?;
        serde_json::to_string(&views).map_err(|e| PluginError::new(format!("task_list: {e}")))
    }
}

// ── TaskUpdate ──

pub struct TaskUpdateTool;

impl TaskUpdateTool {
    /// Factory for registry id `TaskUpdate`.
    ///
    /// # Errors
    /// Never fails; the signature matches [`crate::core::ToolCreateFn`].
    pub fn create(_config: ConfigValue) -> Result<Arc<dyn Tool>, PluginError> {
        Ok(Arc::new(Self))
    }
}

impl Tool for TaskUpdateTool {
    fn name(&self) -> String {
        "task_update".to_string()
    }
    fn description(&self) -> String {
        "Edit a task's title/details/tag/assignee/parent. Pass an empty string to clear a field."
            .to_string()
    }
    fn system_prompt(&self) -> Option<String> {
        Some(TASKS_GUIDANCE.to_string())
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"id":{"type":"string"},"title":{"type":"string"},"details":{"type":"string"},"tag":{"type":"string"},"assignee":{"type":"string"},"parent_id":{"type":"string"}},"required":["id"]}"#.to_string()
    }
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        let args = parse_json(raw)?;
        if arg_str(&args, "id").is_none() {
            return Err(ToolCallError::BadRequest("missing 'id'".to_string()));
        }
        Ok(args)
    }
    fn execute(&self, _ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError> {
        let id =
            arg_str(&args, "id").ok_or_else(|| PluginError::new("task_update: missing 'id'"))?;
        // For clearable fields, distinguish "absent" (keep) from "" (clear):
        // arg_str filters out empty strings, so read those raw.
        let raw_string = |key: &str| {
            args.get(key)
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
        };
        let patch = TaskPatch {
            title: arg_str(&args, "title"),
            details: raw_string("details"),
            tag: raw_string("tag"),
            assignee: raw_string("assignee"),
            parent: raw_string("parent_id"),
        };
        let view = Tasks::open()
            .update(&id, patch)
            .map_err(|e| PluginError::new(format!("task_update: {e}")))?;
        Ok(view_json(&view))
    }
}

// ── TaskResolve ──

pub struct TaskResolveTool;

impl TaskResolveTool {
    /// Factory for registry id `TaskResolve`.
    ///
    /// # Errors
    /// Never fails; the signature matches [`crate::core::ToolCreateFn`].
    pub fn create(_config: ConfigValue) -> Result<Arc<dyn Tool>, PluginError> {
        Ok(Arc::new(Self))
    }
}

impl Tool for TaskResolveTool {
    fn name(&self) -> String {
        "task_resolve".to_string()
    }
    fn description(&self) -> String {
        "Mark a task done. Fails while the task still has open children.".to_string()
    }
    fn system_prompt(&self) -> Option<String> {
        Some(TASKS_GUIDANCE.to_string())
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#.to_string()
    }
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        let args = parse_json(raw)?;
        if arg_str(&args, "id").is_none() {
            return Err(ToolCallError::BadRequest("missing 'id'".to_string()));
        }
        Ok(args)
    }
    fn execute(&self, _ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError> {
        let id =
            arg_str(&args, "id").ok_or_else(|| PluginError::new("task_resolve: missing 'id'"))?;
        let view = Tasks::open()
            .complete(&id)
            .map_err(|e| PluginError::new(format!("task_resolve: {e}")))?;
        Ok(view_json(&view))
    }
}
