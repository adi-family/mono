//! Bundled port of the orchestration capability's employee inbox: the
//! `MessageEmployee` tool (send) and the `EmployeeMessage` trigger
//! (receive). Messages ride a persistent per-employee disk queue, so a
//! send never blocks on the recipient being loaded.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config_value::ConfigValue;
use crate::loop_run_context::LoopRunContext;
use crate::plugin::PluginError;
use crate::queue;
use crate::tool_def::{Tool, ToolCallError, ToolResult};
use crate::trigger::{Trigger, TriggerSend};

/// Send a message to another registered employee's inbox queue.
pub struct MessageEmployeeTool {
    /// When non-empty, restrict legal recipients to employees whose
    /// registration labels match every pair here.
    labels: HashMap<String, String>,
}

impl MessageEmployeeTool {
    /// Factory registered under `adi.workforce.capability.orchestration` /
    /// `MessageEmployee`. Config: `{ labels?: { k: v, ... } }`.
    ///
    /// # Errors
    /// Never fails; the signature matches [`crate::core::ToolCreateFn`].
    pub fn create(config: ConfigValue) -> Result<Arc<dyn Tool>, PluginError> {
        let mut labels = HashMap::new();
        if let Some(ConfigValue::Map(map)) = config.get("labels") {
            for (k, v) in map {
                if let Some(s) = v.as_str() {
                    labels.insert(k.clone(), s.to_string());
                }
            }
        }
        Ok(Arc::new(Self { labels }))
    }
}

impl Tool for MessageEmployeeTool {
    fn name(&self) -> String {
        "message_employee".to_string()
    }
    fn description(&self) -> String {
        "Send a message to another employee. The recipient handles it in their EmployeeMessage trigger.".to_string()
    }
    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"employee":{"type":"string","description":"Recipient employee name (as registered)"},"message":{"type":"string","description":"The message to deliver"}},"required":["employee","message"]}"#.to_string()
    }
    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        ConfigValue::from_json(raw)
            .map_err(|e| ToolCallError::Internal(format!("invalid JSON: {e}")))
    }
    fn execute(&self, ctx: &LoopRunContext, args: ConfigValue) -> Result<String, PluginError> {
        let employee = args
            .get("employee")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::new("message_employee: missing 'employee'"))?
            .to_string();
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::new("message_employee: missing 'message'"))?
            .to_string();
        self.handle(ctx, &employee, &message)
    }
}

impl MessageEmployeeTool {
    fn handle(&self, ctx: &LoopRunContext, employee: &str, message: &str) -> ToolResult {
        // Recipient candidates come from the process-wide employee registry —
        // every WASM module that called `sdk.register(...)` is discoverable.
        // If the tool was configured with `labels`, restrict to that subset.
        let candidates = if self.labels.is_empty() {
            ctx.employee_registry.list()
        } else {
            ctx.employee_registry.list_matching(&self.labels)
        };

        if !candidates.iter().any(|e| e.name == employee) {
            let available = candidates
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(PluginError::new(format!(
                "unknown employee '{employee}'. Available: {available}"
            )));
        }

        let dir = queue::queue_dir(&ctx.workforce_dir, employee, "inbox");
        // Queue payload matches the EmployeeMessage trigger schema — the
        // trigger forwards this line verbatim to the recipient's handler.
        let msg = serde_json::json!({
            "from": ctx.employee,
            "message": message,
        })
        .to_string();
        queue::push_string(&dir, &msg)?;

        Ok(format!("Message sent to {employee}"))
    }
}

/// Blocking watcher over this employee's inbox queue; each received line is
/// forwarded verbatim to the subscribed WASM handler.
pub struct EmployeeMessageTrigger;

impl EmployeeMessageTrigger {
    /// Factory registered under `adi.workforce.capability.orchestration` /
    /// `EmployeeMessage`.
    ///
    /// # Errors
    /// Never fails; the signature matches [`crate::core::TriggerCreateFn`].
    pub fn create(_config: ConfigValue) -> Result<Arc<dyn Trigger>, PluginError> {
        Ok(Arc::new(Self))
    }
}

impl Trigger for EmployeeMessageTrigger {
    fn name(&self) -> String {
        "EmployeeMessage".to_string()
    }

    fn watch(&self, ctx: &LoopRunContext, send: TriggerSend<'_>) {
        let dir = queue::queue_dir(&ctx.workforce_dir, &ctx.employee, "inbox");
        let mut receiver = match queue::QueueReceiver::open(&dir) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[employee-message] failed to open queue: {e}");
                return;
            }
        };

        receiver.recv_blocking(|msg| {
            send(msg.to_string());
        });
    }
}
