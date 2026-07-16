use std::sync::Arc;

use crate::config_value::ConfigValue;
use crate::loop_run_context::LoopRunContext;
use crate::plugin::PluginError;

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters_json: String,
}

pub type ToolOutput = String;
pub type ToolResult = Result<ToolOutput, PluginError>;

#[derive(Debug)]
pub enum ToolCallError {
    /// Bad input from LLM — route back to fix
    BadRequest(String),
    /// Internal tool failure
    Internal(String),
}

impl std::fmt::Display for ToolCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolCallError::BadRequest(msg) => write!(f, "{msg}"),
            ToolCallError::Internal(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<PluginError> for ToolCallError {
    fn from(e: PluginError) -> Self {
        ToolCallError::Internal(e.message)
    }
}

pub trait Tool: Send + Sync {
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn parameters_json(&self) -> String;
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError>;
    fn execute(&self, ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError>;

    /// Optional system-prompt contribution. Returned text is appended to the
    /// loop's system prompt at runtime so a tool can teach the LLM how to
    /// use it without the user having to copy-paste boilerplate into every
    /// employee config.
    ///
    /// The runner deduplicates identical strings, so a family of related
    /// tools (e.g. all the task tools) can return the same shared constant
    /// and the LLM only sees the section once. Default: `None`.
    fn system_prompt(&self) -> Option<String> {
        None
    }
}

impl Tool for Arc<dyn Tool> {
    fn name(&self) -> String {
        self.as_ref().name()
    }
    fn description(&self) -> String {
        self.as_ref().description()
    }
    fn parameters_json(&self) -> String {
        self.as_ref().parameters_json()
    }
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        self.as_ref().parse(raw)
    }
    fn execute(&self, ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError> {
        self.as_ref().execute(ctx, args)
    }
    fn system_prompt(&self) -> Option<String> {
        self.as_ref().system_prompt()
    }
}

pub fn to_tool_def(tool: &dyn Tool) -> ToolDef {
    ToolDef {
        name: tool.name(),
        description: tool.description(),
        parameters_json: tool.parameters_json(),
    }
}
