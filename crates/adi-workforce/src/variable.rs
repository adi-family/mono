use crate::plugin::PluginError;

pub trait VariablePlugin: Send + Sync {
    fn kind(&self) -> &str;
    fn resolve(&self, value: &str) -> Result<String, PluginError>;
}
