//! adi-mcp — the adi platform MCP server, as a library.
//!
//! [`AdiMcp`] is a single MCP handler whose tool set is assembled at construction time from
//! an enabled [`FeatureSet`]. Each feature module (`tasks`, `projects`, `files`, `status`)
//! contributes a *named* `ToolRouter` through a `#[tool_router(router = …)]` inherent impl
//! block on `AdiMcp`; [`AdiMcp::new`] merges only the routers whose feature is enabled. That
//! is what lets one binary expose a scoped tool set chosen at launch:
//!
//! ```text
//! adi-mcp --features "tasks,projects"   # only the tasks_* and projects_* tools
//! ```
//!
//! The transport is stdio (see the `adi-mcp` binary): an agent spawns the process and speaks
//! MCP over its stdin/stdout.

pub mod features;
pub mod server;

// Each of these declares one `#[tool_router(router = …)]` impl block on `AdiMcp`.
mod files;
mod projects;
mod status;
mod tasks;

pub use features::{Feature, FeatureSet};
pub use server::AdiMcp;
