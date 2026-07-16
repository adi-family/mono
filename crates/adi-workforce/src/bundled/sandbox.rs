//! Bundled port of `adi.workforce.filesystem.sandbox`.
//!
//! Exposes:
//!   - `Init({ id, preloadTools? })` function — allocates a deterministic
//!     sandbox directory `{workforce_dir}/.sandboxes/{id}`, symlinks tools,
//!     returns `{id, realPath}`. Safe to call repeatedly for the same id.
//!   - `Cleanup({ id })` function — removes the sandbox directory.
//!   - `Sandbox({ id })` filesystem — resolves the loop's workdir to the
//!     sandbox's real path. Registered globally via a static registry so
//!     an `Init` call from code can be referenced later by filesystem
//!     resolution (different plugin instances, same process).
//!
//! Why a static registry: filesystem `resolve_workdir()` has no context
//! beyond its own config. The id → real-path mapping has to live
//! somewhere shared. We key strictly by id so config churn can't desync
//! the mapping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::config_value::ConfigValue;
use crate::filesystem::Filesystem;
use crate::loop_run_context::LoopRunContext;
use crate::plugin::PluginError;
use crate::tool_def::{Tool, ToolCallError};

// ── Static registry: id → real path ──

static REGISTRY: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, PathBuf>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register(id: &str, path: &Path) {
    let _ = registry()
        .lock()
        .map(|mut m| m.insert(id.to_string(), path.to_path_buf()));
}

fn lookup(id: &str) -> Option<PathBuf> {
    registry().lock().ok().and_then(|m| m.get(id).cloned())
}

fn forget(id: &str) -> Option<PathBuf> {
    registry().lock().ok().and_then(|mut m| m.remove(id))
}

// ── Directory layout ──

/// Deterministic path for a given id. One workforce_dir → one sandboxes
/// tree, so different employees in the same workforce can share ids if
/// they coordinate.
fn sandbox_path(workforce_dir: &Path, id: &str) -> PathBuf {
    workforce_dir.join(".sandboxes").join(id)
}

/// Reject path-escaping ids up front so a caller can't point sandbox_path
/// outside `.sandboxes/`.
fn validate_id(id: &str) -> Result<(), PluginError> {
    if id.is_empty() {
        return Err(PluginError::new("sandbox id must not be empty"));
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(PluginError::new(format!(
            "sandbox id '{id}' contains path separators or '..'"
        )));
    }
    Ok(())
}

// ── Init tool ──
//
// Not LLM-callable by convention (no tool description contract). The TS
// SDK calls it via `sdk.plugin('...').functions.Init({...})` which maps
// onto `call_tool` — same dispatch, just named differently on the caller
// side.

pub struct InitTool;

impl InitTool {
    pub fn create(_config: ConfigValue) -> Result<Arc<dyn Tool>, PluginError> {
        Ok(Arc::new(Self))
    }
}

impl Tool for InitTool {
    fn name(&self) -> String {
        "Init".to_string()
    }
    fn description(&self) -> String {
        "Create (or reuse) a sandbox directory and return {id, realPath}.".to_string()
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"id":{"type":"string"},"preloadTools":{"type":"array","items":{"type":"string"}}},"required":["id"]}"#.to_string()
    }
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        ConfigValue::from_json(raw)
            .map_err(|e| ToolCallError::Internal(format!("invalid JSON: {e}")))
    }
    fn execute(&self, ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::new("Init: missing 'id'"))?;
        validate_id(id)?;

        let preload_tools: Vec<String> = args
            .get("preloadTools")
            .and_then(|v| v.as_list())
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let dir = sandbox_path(&ctx.workforce_dir, id);
        let bin_dir = dir.join("bin");
        let tmp_dir = dir.join("tmp");

        for d in [dir.as_path(), &bin_dir, &tmp_dir] {
            std::fs::create_dir_all(d).map_err(|e| {
                PluginError::new(format!(
                    "sandbox Init: mkdir failed for {}: {e}",
                    d.display()
                ))
            })?;
        }

        // sh is always linked; everything else is opt-in per caller.
        let mut to_link: Vec<&str> = vec!["sh"];
        for t in &preload_tools {
            if t != "sh" && !to_link.contains(&t.as_str()) {
                to_link.push(t.as_str());
            }
        }

        for tool_name in &to_link {
            let link_path = bin_dir.join(tool_name);
            if link_path.exists() {
                continue;
            }
            let tool_path = resolve_tool_path(tool_name).map_err(PluginError::new)?;
            symlink(&tool_path, &link_path).map_err(PluginError::new)?;
        }

        let real = dir.canonicalize().unwrap_or(dir.clone());
        register(id, &real);

        let resp = serde_json::json!({
            "id": id,
            "realPath": real.to_string_lossy(),
        });
        Ok(resp.to_string())
    }
}

// ── Cleanup tool ──

pub struct CleanupTool;

impl CleanupTool {
    pub fn create(_config: ConfigValue) -> Result<Arc<dyn Tool>, PluginError> {
        Ok(Arc::new(Self))
    }
}

impl Tool for CleanupTool {
    fn name(&self) -> String {
        "Cleanup".to_string()
    }
    fn description(&self) -> String {
        "Remove a sandbox directory. Returns {removed: bool}.".to_string()
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#.to_string()
    }
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        ConfigValue::from_json(raw)
            .map_err(|e| ToolCallError::Internal(format!("invalid JSON: {e}")))
    }
    fn execute(&self, ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::new("Cleanup: missing 'id'"))?;
        validate_id(id)?;

        let path = forget(id).unwrap_or_else(|| sandbox_path(&ctx.workforce_dir, id));
        let removed = path.is_dir() && std::fs::remove_dir_all(&path).is_ok();

        Ok(serde_json::json!({ "removed": removed }).to_string())
    }
}

// ── Sandbox filesystem ──

pub struct SandboxFilesystem {
    id: String,
}

impl SandboxFilesystem {
    pub fn create(config: ConfigValue) -> Result<Arc<dyn Filesystem>, PluginError> {
        let id = config
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::new("Sandbox filesystem: missing 'id'"))?
            .to_string();
        validate_id(&id)?;
        Ok(Arc::new(Self { id }))
    }
}

impl Filesystem for SandboxFilesystem {
    fn resolve_workdir(&self) -> Result<PathBuf, PluginError> {
        lookup(&self.id).ok_or_else(|| {
            PluginError::new(format!(
                "sandbox '{}' not initialized — call functions.Init({{id: '{}'}}) first",
                self.id, self.id
            ))
        })
    }
}

// ── Shared helpers ──

fn resolve_tool_path(name: &str) -> Result<PathBuf, String> {
    let output = std::process::Command::new("which")
        .arg(name)
        .output()
        .map_err(|e| format!("failed to run `which {name}`: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "tool '{name}' not found (which exited {})",
            output.status.code().unwrap_or(-1)
        ));
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        return Err(format!("tool '{name}' resolved to empty path"));
    }
    Ok(PathBuf::from(path))
}

fn symlink(tool_path: &Path, link_path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(tool_path, link_path).map_err(|e| {
            format!(
                "symlink {} -> {}: {e}",
                link_path.display(),
                tool_path.display()
            )
        })
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(tool_path, link_path)
            .map(|_| ())
            .map_err(|e| {
                format!(
                    "copy {} -> {}: {e}",
                    tool_path.display(),
                    link_path.display()
                )
            })
    }
}

// ── Registration ──

pub fn register_plugin() -> crate::core::PluginEntry {
    crate::core::PluginEntry::new()
        .tool("Init", InitTool::create)
        .tool("Cleanup", CleanupTool::create)
        .filesystem("Sandbox", SandboxFilesystem::create)
}

