//! Runner plugin trait: resolves a runner config and builds the [`LlmBackend`]
//! a loop session talks to. Ported from the old workforce-core, minus the
//! native-loop `run()` entry point — in this engine the agentic loop is always
//! driven from the TS side of the WASM boundary (`loop-llm` / `loop-tool`),
//! so a runner only ever needs to produce a backend.

use crate::config_value::ConfigValue;
use crate::llm::LlmBackend;
use crate::plugin::PluginError;

pub trait LoopRunnerPlugin: Send + Sync {
    /// The runner id this plugin answers to (e.g. `ClaudeCodeApi`).
    fn kind(&self) -> &str;

    /// Normalize/complete the raw runner config (fill defaults, resolve
    /// aliases). The resolved value is what `build_backend` receives and
    /// what `maxTurns`/`maxTokens` are read from at loop-init.
    fn resolve(&self, config: ConfigValue) -> Result<ConfigValue, PluginError>;

    fn build_backend(
        &self,
        resolved_config: &ConfigValue,
    ) -> Result<Box<dyn LlmBackend>, PluginError>;
}

/// A runner resolved for one loop session: the plugin plus its resolved config.
pub struct ResolvedRunner {
    pub plugin: std::sync::Arc<dyn LoopRunnerPlugin>,
    pub resolved_config: ConfigValue,
}

impl std::fmt::Debug for ResolvedRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedRunner")
            .field("kind", &self.plugin.kind())
            .finish_non_exhaustive()
    }
}
