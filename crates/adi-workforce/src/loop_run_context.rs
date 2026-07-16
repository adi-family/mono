use std::path::PathBuf;
use std::sync::Arc;

use crate::employee_registry::EmployeeRegistry;

pub struct LoopRunContext {
    pub id: String,
    pub employee: String,
    pub loop_id: String,
    pub workforce_dir: PathBuf,
    pub workdir: PathBuf,
    pub max_turns: usize,
    /// Registry of WASM-registered employees in this process. Tools such as
    /// orchestration's `MessageEmployee` look up legal recipients here instead
    /// of carrying their own employee list in config.
    pub employee_registry: Arc<EmployeeRegistry>,
    /// Caller-supplied metadata attached at loop-init time (SDK
    /// `LoopConfig.metadata`). Opaque to the host; intended for
    /// middlewares, triggers, and observability to filter or group runs
    /// by caller-defined keys (e.g. `taskId`). `Null` when not provided.
    pub metadata: serde_json::Value,
}

impl LoopRunContext {
    pub fn employee_dir(&self) -> PathBuf {
        self.workforce_dir.join(&self.employee)
    }

    pub fn other_employee_dir(&self, codename: &str) -> PathBuf {
        self.workforce_dir.join(codename)
    }
}
