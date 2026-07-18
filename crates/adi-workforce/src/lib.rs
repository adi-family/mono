//! adi-workforce — the WASM-employee agent engine, ported from the old
//! adi-family repo's `workforce-core`.
//!
//! An agent ("employee") is written in TypeScript against the workforce SDK,
//! compiled to a WebAssembly *component* (esbuild → jco, WIT world
//! `loop-script`), and driven by this crate's wasmtime host:
//!
//! - [`WasmEmployee`] loads a component, reads its `sdk.register(...)`
//!   identity, runs `main()` to collect trigger subscriptions, and
//!   dispatches events into handlers.
//! - The TS side drives each `sdk.loop(...).run()` itself; the host is an
//!   LLM/tool execution service exposed over the WIT `host` interface
//!   (`loop-init` / `loop-llm` / `loop-tool` / `loop-finish`).
//! - Capabilities (tools, runners, filesystems, triggers) live in
//!   [`bundled`] — statically compiled in, registered under the same
//!   plugin-id strings the old dlopen'd plugins used, so existing agent
//!   sources keep working unchanged.

pub mod bundled;
pub mod config_value;
pub mod core;
pub mod dispatch;
pub mod employee_registry;
pub mod filesystem;
pub mod llm;
pub mod loop_run_context;
pub mod loop_runner_plugin;
pub mod plugin;
pub mod queue;
pub mod sdk_log;
pub mod stats;
pub mod tool_def;
pub mod trigger;
pub mod variable;
pub mod wasm_config_loader;

pub use config_value::ConfigValue;
pub use core::{Core, PluginEntry};
pub use dispatch::{dispatch_message, DispatchOutcome};
pub use employee_registry::{EmployeeRegistration, EmployeeRegistry};
pub use filesystem::Filesystem;
pub use llm::{LlmBackend, LlmRequest, LlmResponse};
pub use loop_run_context::LoopRunContext;
pub use loop_runner_plugin::{LoopRunnerPlugin, ResolvedRunner};
pub use plugin::PluginError;
pub use tool_def::{Tool, ToolCallError, ToolDef, ToolResult};
pub use trigger::{Trigger, TriggerSend};
pub use variable::VariablePlugin;
pub use wasm_config_loader::{HostState, Subscription, WasmEmployee};
