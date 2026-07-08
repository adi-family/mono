//! The `status` feature: a read-only view of adi platform service state, wrapping
//! [`adi_core::Adi::report`] — the same aggregate the GUI polls. It only *reads* status
//! (status files + liveness checks); it never enables, disables, or restarts anything, so it
//! is safe to hand an agent without risking the protected DNS / front-door services.

use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, tool, tool_router};

use crate::server::{AdiMcp, json_result};

#[tool_router(router = status_router, vis = "pub")]
impl AdiMcp {
    #[tool(description = "Report the live status of adi platform services (read-only)")]
    async fn status_report(&self) -> Result<CallToolResult, McpError> {
        let report = adi_core::Adi::new().report();
        json_result(&report)
    }
}
