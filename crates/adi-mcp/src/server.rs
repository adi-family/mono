//! The [`AdiMcp`] server: one MCP handler whose tool set is assembled at construction time
//! from the enabled [`FeatureSet`]. Each feature contributes a named `ToolRouter` (see the
//! `#[tool_router(router = …)]` impl blocks in the feature modules); [`AdiMcp::new`] merges
//! only the routers for enabled features into `self.tool_router`, which `#[tool_handler]`
//! then serves. The struct also carries the [`FeatureSet`] so `get_info` can advertise which
//! groups are live.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::{ErrorData as McpError, ServerHandler, tool_handler};
use serde::Serialize;

use crate::features::{Feature, FeatureSet};

/// The adi platform MCP server. Cheap to clone; all real state lives on disk (the tools open
/// their stores on demand), so the struct carries only its feature set and tool router.
#[derive(Debug, Clone)]
pub struct AdiMcp {
    features: FeatureSet,
    tool_router: ToolRouter<AdiMcp>,
}

impl AdiMcp {
    /// Build a server exposing exactly the tools of the enabled `features`.
    ///
    /// The feature modules each generate an inherent `Self::<feature>_router()` via
    /// `#[tool_router(router = …)]` holding *all* of that feature's tools. For each enabled
    /// feature we take its full router, drop any tool the [`FeatureSet`] did not select (a
    /// `feature[tool,…]` selector), and merge the rest in.
    #[must_use]
    pub fn new(features: FeatureSet) -> Self {
        let mut tool_router = ToolRouter::new();
        if features.contains(Feature::Tasks) {
            tool_router.merge(select(Self::tasks_router(), Feature::Tasks, &features));
        }
        if features.contains(Feature::Projects) {
            tool_router.merge(select(Self::projects_router(), Feature::Projects, &features));
        }
        if features.contains(Feature::Files) {
            tool_router.merge(select(Self::files_router(), Feature::Files, &features));
        }
        if features.contains(Feature::Status) {
            tool_router.merge(select(Self::status_router(), Feature::Status, &features));
        }
        Self {
            features,
            tool_router,
        }
    }
}

/// Drop from `router` every tool of `feature` that `features` did not select, leaving only the
/// enabled ones. A bare feature (no `[...]` selector) keeps all its tools.
fn select(mut router: ToolRouter<AdiMcp>, feature: Feature, features: &FeatureSet) -> ToolRouter<AdiMcp> {
    for short in feature.tools() {
        if !features.includes_tool(feature, short) {
            router.remove_route(&feature.full_tool_name(short));
        }
    }
    router
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AdiMcp {
    fn get_info(&self) -> ServerInfo {
        let enabled = self
            .features
            .iter()
            .map(Feature::name)
            .collect::<Vec<_>>()
            .join(", ");
        let enabled = if enabled.is_empty() {
            "(none)".to_string()
        } else {
            enabled
        };
        let instructions = format!(
            "adi platform tools for agents. Enabled feature groups: {enabled}. \
             Tool names are prefixed by group (e.g. tasks_create, projects_list, files_read, \
             status_report). Every tool returns JSON text unless noted."
        );
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(instructions)
    }
}

/// Serialize `value` as pretty JSON text content — the uniform success shape most tools
/// return so agents get structured, parseable output.
///
/// # Errors
/// [`McpError::internal_error`] if `value` fails to serialize (should not happen for the
/// plain data types the tools return).
pub(crate) fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("failed to serialize result: {e}"), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// A plain-text success result — for tools whose reply is a human-readable acknowledgement
/// (e.g. "deleted task t3") or raw file contents rather than a JSON document.
pub(crate) fn text_result(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(text.into())])
}

/// Map an internal (non-client) error to an [`McpError::internal_error`], prefixed with
/// `context` so the agent sees which operation failed.
pub(crate) fn internal(context: &str, e: impl std::fmt::Display) -> McpError {
    McpError::internal_error(format!("{context}: {e}"), None)
}
