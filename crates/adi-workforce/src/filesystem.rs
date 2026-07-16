//! Filesystem plugin type.
//!
//! A `Filesystem` resolves a loop's working directory on demand. The SDK
//! passes a `FilesystemRef { pluginId, fsId, config }` in `LoopConfig`;
//! the host calls `Filesystem::resolve_workdir` to get the actual path
//! to use as the loop's workdir (overriding the default per-employee dir).
//!
//! This is the entry point for sandboxed work areas — the filesystem
//! implementation owns how that directory is created and cleaned up.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config_value::ConfigValue;
use crate::plugin::PluginError;

pub trait Filesystem: Send + Sync {
    /// Return the absolute path to use as the loop's working directory.
    ///
    /// Called once per loop run, before the runner starts. Errors here
    /// abort the loop before any tool fires.
    fn resolve_workdir(&self) -> Result<PathBuf, PluginError>;
}

pub type FilesystemCreateFn = fn(ConfigValue) -> Result<Arc<dyn Filesystem>, PluginError>;
