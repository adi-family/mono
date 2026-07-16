//! The engine's capability registry. Ported from the old workforce-core `Core`,
//! trimmed to the surfaces the WASM path uses (tools, triggers, runners,
//! filesystems, variables). The old repo filled this registry by dlopen-ing
//! capability plugins; here everything is bundled — [`crate::bundled::core`]
//! builds a `Core` with every built-in capability statically registered.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config_value::ConfigValue;
use crate::employee_registry::EmployeeRegistry;
use crate::filesystem::FilesystemCreateFn;
use crate::loop_runner_plugin::LoopRunnerPlugin;
use crate::plugin::PluginError;
use crate::tool_def::Tool;
use crate::trigger::Trigger;
use crate::variable::VariablePlugin;

pub type ToolCreateFn = fn(ConfigValue) -> Result<Arc<dyn Tool>, PluginError>;
pub type TriggerCreateFn = fn(ConfigValue) -> Result<Arc<dyn Trigger>, PluginError>;
pub type VariableCreateFn = fn(ConfigValue) -> Result<Arc<dyn VariablePlugin>, PluginError>;
pub type RunnerCreateFn = fn(ConfigValue) -> Result<Arc<dyn LoopRunnerPlugin>, PluginError>;

/// One capability namespace (what the SDK calls `sdk.plugin("<id>")`): named
/// factories for each kind of thing the namespace provides.
#[derive(Default)]
pub struct PluginEntry {
    pub tools: HashMap<String, ToolCreateFn>,
    pub triggers: HashMap<String, TriggerCreateFn>,
    pub variables: HashMap<String, VariableCreateFn>,
    pub runners: HashMap<String, RunnerCreateFn>,
    pub filesystems: HashMap<String, FilesystemCreateFn>,
}

impl std::fmt::Debug for PluginEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginEntry")
            .field("tools", &self.tools.keys())
            .field("triggers", &self.triggers.keys())
            .field("variables", &self.variables.keys())
            .field("runners", &self.runners.keys())
            .field("filesystems", &self.filesystems.keys())
            .finish()
    }
}

impl PluginEntry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn tool(mut self, name: &str, f: ToolCreateFn) -> Self {
        self.tools.insert(name.to_string(), f);
        self
    }

    #[must_use]
    pub fn trigger(mut self, name: &str, f: TriggerCreateFn) -> Self {
        self.triggers.insert(name.to_string(), f);
        self
    }

    #[must_use]
    pub fn variable(mut self, name: &str, f: VariableCreateFn) -> Self {
        self.variables.insert(name.to_string(), f);
        self
    }

    #[must_use]
    pub fn runner(mut self, name: &str, f: RunnerCreateFn) -> Self {
        self.runners.insert(name.to_string(), f);
        self
    }

    #[must_use]
    pub fn filesystem(mut self, name: &str, f: FilesystemCreateFn) -> Self {
        self.filesystems.insert(name.to_string(), f);
        self
    }
}

/// Process-wide engine state: the capability registry plus the registry of
/// live WASM employees.
#[derive(Debug, Default)]
pub struct Core {
    pub plugin_registry: HashMap<String, PluginEntry>,
    pub employee_registry: Arc<EmployeeRegistry>,
}

impl Core {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_plugin(&mut self, id: &str, entry: PluginEntry) {
        self.plugin_registry.insert(id.to_string(), entry);
    }
}
