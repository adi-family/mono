//! Bundled port of `adi.workforce.variable.env`.
//!
//! `Env` is surfaced as a tool (not a variable plugin) because the SDK's
//! `.functions.Env(...)` helper resolves through the host's `call_tool`
//! path with the plugin-qualified name.

use std::sync::Arc;

use crate::config_value::ConfigValue;
use crate::loop_run_context::LoopRunContext;
use crate::plugin::PluginError;
use crate::tool_def::{Tool, ToolCallError};

pub struct Env;

impl Env {
    /// Factory registered under `adi.workforce.variable.env` / `Env`.
    ///
    /// # Errors
    /// Never fails; the signature matches [`crate::core::ToolCreateFn`].
    pub fn create(_config: ConfigValue) -> Result<Arc<dyn Tool>, PluginError> {
        Ok(Arc::new(Env))
    }
}

impl Tool for Env {
    fn name(&self) -> String {
        "Env".to_string()
    }
    fn description(&self) -> String {
        "Resolve a value from an environment variable.".to_string()
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"value":{"type":"string","description":"Environment variable name"}},"required":["value"]}"#.to_string()
    }
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        ConfigValue::from_json(raw)
            .map_err(|e| ToolCallError::Internal(format!("invalid JSON: {e}")))
    }
    fn execute(&self, _ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError> {
        let key = args
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::new("Env: missing 'value'"))?;
        let resolved = std::env::var(key)
            .map_err(|_| PluginError::new(format!("env var not found: {key}")))?;
        Ok(serde_json::json!({ "resolved": resolved }).to_string())
    }
}
