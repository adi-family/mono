//! WASM Config Loader
//!
//! Loads employee configs from compiled WASM components (TypeScript → esbuild → jco → .wasm).
//! Implements the WIT host interface by delegating to real Rust plugin trait implementations.
//!
//! Runtime model: the TS SDK drives each `sdk.loop(...).run()` call
//! synchronously. It calls `loop-init` once to set up a session
//! (resolve filesystem, instantiate tools, build LLM backend), then
//! alternates `loop-llm` (one turn of conversation) and `loop-tool`
//! (one tool execution) until the model returns a final answer, then
//! `loop-finish` tears the session down. All middleware/lifetime logic
//! runs in TS — the host is just an LLM/tool execution service, so
//! there is no wasmtime reentry at all.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use wasmtime::component::*;
use wasmtime::{Config, Engine, Store};

use crate::config_value::ConfigValue;
use crate::core::Core;
use crate::loop_runner_plugin::ResolvedRunner;
use crate::employee_registry::EmployeeRegistration;
use crate::filesystem::Filesystem;
use crate::llm::{
    AssistantBlock, AssistantTurn, LlmBackend, LlmRequest, Turn, UserBlock, UserTurn,
};
use crate::loop_run_context::LoopRunContext;
use crate::tool_def::{to_tool_def, Tool, ToolDef};

// ── WIT bindings ──

bindgen!({
    world: "loop-script",
    path: "wit/workforce.wit",
});

// ── JSON deserialization ──

#[derive(Debug, Deserialize)]
struct WasmToolDef {
    plugin: String,
    tool: String,
    #[serde(default)]
    config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WasmRunnerDef {
    #[serde(rename = "pluginId")]
    plugin_id: String,
    #[serde(rename = "runnerId")]
    runner_id: String,
    #[serde(default)]
    config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WasmFilesystemDef {
    #[serde(rename = "pluginId")]
    plugin_id: String,
    #[serde(rename = "fsId")]
    fs_id: String,
    #[serde(default)]
    config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WasmLoopConfig {
    name: String,
    system: Option<String>,
    tools: Option<Vec<WasmToolDef>>,
    runner: Option<WasmRunnerDef>,
    filesystem: Option<WasmFilesystemDef>,
    /// Opt-in structured output. When present, the host advertises a
    /// synthetic tool to the LLM whose schema is `parametersJson`.
    /// When the model calls it, the loop terminates and the tool args
    /// are returned to the SDK as `decision` on the llm response.
    schema: Option<WasmSchemaDef>,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize, Clone)]
struct WasmSchemaDef {
    /// Synthetic tool name advertised to the LLM. Defaults to
    /// `record_decision`. Override when a loop already exposes a tool
    /// with that name.
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "parametersJson")]
    parameters_json: String,
}

const DEFAULT_MAX_TOKENS: u64 = 16384;
const DEFAULT_MAX_TURNS: u64 = 1000;

// ── JSON → ConfigValue conversion ──

fn json_to_config(value: &serde_json::Value) -> ConfigValue {
    match value {
        serde_json::Value::Null => ConfigValue::Null,
        serde_json::Value::Bool(b) => ConfigValue::Bool(*b),
        serde_json::Value::Number(n) => ConfigValue::Num(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => ConfigValue::Str(s.clone()),
        serde_json::Value::Array(arr) => {
            ConfigValue::List(arr.iter().map(json_to_config).collect())
        }
        serde_json::Value::Object(map) => ConfigValue::Map(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_config(v)))
                .collect(),
        ),
    }
}

// ── Subscription ──

pub struct Subscription {
    pub name: String,
    pub plugin_id: String,
    pub trigger_name: String,
    pub config: ConfigValue,
}

// ── Loop session ──
//
// Created in `loop_init`, owned by `HostState.loop_sessions` until the
// matching `loop_finish`. The filesystem `Arc` is held so sandboxed
// workdirs stay alive for the session's entire duration; dropping it in
// `loop_finish` is the cleanup hook.

struct LoopSession {
    tools: Vec<Arc<dyn Tool>>,
    tool_defs: Vec<ToolDef>,
    llm: Box<dyn LlmBackend>,
    ctx: LoopRunContext,
    system_prompt: String,
    max_tokens: usize,
    loop_id: String,
    /// Monotonic tool-call counter for this session, used as a `turn`
    /// approximation in the observability log. Starts at 0; incremented
    /// each `loop_tool` call regardless of success.
    tool_call_seq: usize,
    /// Filesystem impl held for session lifetime so sandbox dirs (and
    /// other resources) aren't reclaimed mid-loop. `None` when the loop
    /// uses the default per-employee workdir.
    _filesystem: Option<Arc<dyn Filesystem>>,
    /// Name of the synthetic decision tool advertised to the LLM, if
    /// the session was configured with a `schema`. When the model
    /// emits a `tool_use` with this name, `loop_llm` lifts its args
    /// into the response's `decision` field so the SDK terminates.
    decision_tool: Option<String>,
}

// ── Host state ──

pub struct HostState {
    core: Arc<Core>,
    /// Display name set from `sdk.register(...)`. Overwritten at init
    /// time — not stable for filesystem paths.
    employee_id: String,
    /// Immutable filesystem slug derived from the wasm file stem. The
    /// daemon writes `.status`, `trigger/`, `loop/`, `tool_calls.jsonl`
    /// under `<workforce_dir>/<slug>/` — so the host log persister has
    /// to use the slug too, otherwise `sdk_log.jsonl` ends up orphaned
    /// under the display-name dir.
    employee_slug: String,
    workforce_dir: PathBuf,
    subscriptions: Vec<Subscription>,
    /// Active loop sessions keyed by id returned from `loop_init`.
    loop_sessions: HashMap<String, LoopSession>,
    /// Stack of loop ids for nested `sdk.loop(...).run()` calls. The top
    /// of the stack is what `get-context("loop_id")` returns. The SDK
    /// currently runs loops sequentially, so this is a 1-element stack
    /// almost always — the stack model just keeps the invariant honest
    /// if user code ever nests.
    loop_stack: Vec<String>,
}

impl HostState {
    /// Canonical employee name (as registered via `sdk.register(...)`).
    pub fn employee_id(&self) -> &str {
        &self.employee_id
    }

    fn new(core: Arc<Core>, employee_id: &str, workforce_dir: &Path) -> Self {
        Self {
            core,
            employee_id: employee_id.to_string(),
            employee_slug: employee_id.to_string(),
            workforce_dir: workforce_dir.to_path_buf(),
            subscriptions: Vec::new(),
            loop_sessions: HashMap::new(),
            loop_stack: Vec::new(),
        }
    }

    fn create_tool(
        &self,
        plugin_id: &str,
        tool_name: &str,
        config: ConfigValue,
    ) -> Result<Arc<dyn Tool>, String> {
        let entry = self
            .core
            .plugin_registry
            .get(plugin_id)
            .ok_or_else(|| format!("plugin '{}' not found", plugin_id))?;
        let factory = entry
            .tools
            .get(tool_name)
            .ok_or_else(|| format!("tool '{}' not found in plugin '{}'", tool_name, plugin_id))?;
        factory(config)
            .map_err(|e| format!("failed to create tool {}.{}: {}", plugin_id, tool_name, e))
    }

    fn create_filesystem(
        &self,
        plugin_id: &str,
        fs_id: &str,
        config: ConfigValue,
    ) -> Result<Arc<dyn Filesystem>, String> {
        let entry = self
            .core
            .plugin_registry
            .get(plugin_id)
            .ok_or_else(|| format!("plugin '{}' not found", plugin_id))?;
        let factory = entry
            .filesystems
            .get(fs_id)
            .ok_or_else(|| format!("filesystem '{}' not found in plugin '{}'", fs_id, plugin_id))?;
        factory(config)
            .map_err(|e| format!("failed to create filesystem {}.{}: {}", plugin_id, fs_id, e))
    }

    fn create_runner(
        &self,
        plugin_id: &str,
        runner_id: &str,
        config: ConfigValue,
    ) -> Result<ResolvedRunner, String> {
        if let Some(entry) = self.core.plugin_registry.get(plugin_id) {
            if let Some(factory) = entry.runners.get(runner_id) {
                let plugin =
                    factory(config.clone()).map_err(|e| format!("runner factory error: {}", e))?;
                let resolved_config = plugin
                    .resolve(config)
                    .map_err(|e| format!("runner resolve error: {}", e))?;
                return Ok(ResolvedRunner {
                    plugin,
                    resolved_config,
                });
            }
        }

        Err(format!(
            "runner '{}' not found in plugin '{}'",
            runner_id, plugin_id
        ))
    }

    /// Tool lookup across every active loop session. Used by plain
    /// `call-tool` for plugin function calls (e.g. `functions.Init`).
    /// Caller iterates so borrow rules don't trip on the map while
    /// running the tool.
    fn find_tool_across_sessions(&self, name: &str) -> Option<Arc<dyn Tool>> {
        for session in self.loop_sessions.values() {
            for tool in &session.tools {
                if tool.name() == name {
                    return Some(tool.clone());
                }
            }
        }
        None
    }
}

// ── Turn JSON ↔ Rust model ──
//
// Wire format matches what sdk/index.ts emits when driving the loop
// (see `TurnMsg`). `tool_use.input` and `tool_result.content` are the
// main asymmetry: `input` is an arbitrary JSON value on the wire but
// `arguments: String` on the Rust side (we re-stringify it back to the
// LLM backend verbatim); `tool_result.content` is a string on both sides.

#[derive(Debug, Deserialize)]
struct WireTurn {
    role: String,
    #[serde(default)]
    blocks: Vec<serde_json::Value>,
}

fn parse_turns(turns_json: &str) -> Result<Vec<Turn>, String> {
    let raw: Vec<WireTurn> =
        serde_json::from_str(turns_json).map_err(|e| format!("turns_json parse: {e}"))?;

    let mut turns = Vec::with_capacity(raw.len());
    for t in raw {
        match t.role.as_str() {
            "user" => {
                let mut blocks = Vec::new();
                for b in t.blocks {
                    let ty = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match ty {
                        "text" => {
                            let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            blocks.push(UserBlock::Text(text.to_string()));
                        }
                        "tool_result" => {
                            let tool_use_id = b
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let content = b
                                .get("content")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            blocks.push(UserBlock::ToolResult {
                                tool_use_id,
                                content,
                            });
                        }
                        other => {
                            return Err(format!("unknown user block type '{other}'"));
                        }
                    }
                }
                turns.push(Turn::User(UserTurn { blocks }));
            }
            "assistant" => {
                let mut blocks = Vec::new();
                for b in t.blocks {
                    let ty = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match ty {
                        "text" => {
                            let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            blocks.push(AssistantBlock::Text(text.to_string()));
                        }
                        "thinking" => {
                            let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            let signature = b
                                .get("signature")
                                .and_then(|v| v.as_str())
                                .map(str::to_string);
                            let redacted =
                                b.get("redacted").and_then(|v| v.as_bool()).unwrap_or(false);
                            blocks.push(AssistantBlock::Thinking {
                                text: text.to_string(),
                                signature,
                                redacted,
                            });
                        }
                        "tool_use" => {
                            let id = b
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = b
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            // `input` comes over as an arbitrary JSON value
                            // (object typically). The LLM backend wants the
                            // original string, so re-stringify.
                            let arguments = match b.get("input") {
                                Some(v) => serde_json::to_string(v).unwrap_or_else(|_| "{}".into()),
                                None => "{}".into(),
                            };
                            blocks.push(AssistantBlock::ToolUse {
                                id,
                                name,
                                arguments,
                            });
                        }
                        other => {
                            return Err(format!("unknown assistant block type '{other}'"));
                        }
                    }
                }
                turns.push(Turn::Assistant(AssistantTurn { blocks }));
            }
            other => return Err(format!("unknown turn role '{other}'")),
        }
    }
    Ok(turns)
}

fn serialize_llm_response(
    turn: &AssistantTurn,
    usage_json: serde_json::Value,
    decision_tool: Option<&str>,
) -> String {
    let mut decision: Option<serde_json::Value> = None;
    let blocks: Vec<serde_json::Value> = turn
        .blocks
        .iter()
        .map(|b| match b {
            AssistantBlock::Text(text) => serde_json::json!({ "type": "text", "text": text }),
            AssistantBlock::Thinking {
                text,
                signature,
                redacted,
            } => {
                let mut obj = serde_json::json!({
                    "type": "thinking",
                    "text": text,
                    "redacted": redacted,
                });
                if let Some(sig) = signature {
                    obj["signature"] = serde_json::Value::String(sig.clone());
                }
                obj
            }
            AssistantBlock::ToolUse {
                id,
                name,
                arguments,
            } => {
                // `arguments` is a JSON string on our side; emit it as a
                // parsed value on the wire so TS can forward it as the
                // tool's real argument object.
                let input: serde_json::Value =
                    serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
                // If this tool_use is the loop's synthetic decision
                // tool, lift its args to the top-level `decision` field
                // so the SDK can terminate the loop with a typed value
                // instead of routing through `loop_tool`. First match
                // wins — prompt guidance says "call exactly once" but
                // a runaway model still only yields one decision.
                if decision.is_none()
                    && decision_tool.is_some_and(|n| n == name.as_str())
                {
                    decision = Some(input.clone());
                }
                serde_json::json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input,
                })
            }
        })
        .collect();

    let mut body = serde_json::json!({
        "blocks": blocks,
        "usage": usage_json,
    });
    if let Some(d) = decision {
        body["decision"] = d;
    }
    body.to_string()
}

// ── Host impl ──

impl adi::workforce::host::Host for HostState {
    fn call_tool(&mut self, name: String, args_json: String) -> Result<String, String> {
        let args: serde_json::Value = serde_json::from_str(&args_json).unwrap_or_default();
        let config = json_to_config(&args);

        let ctx = LoopRunContext {
            id: "wasm-call".to_string(),
            employee: self.employee_id.clone(),
            loop_id: self
                .loop_stack
                .last()
                .cloned()
                .unwrap_or_else(|| "direct".to_string()),
            workforce_dir: self.workforce_dir.clone(),
            workdir: self.workforce_dir.join(&self.employee_id),
            max_turns: 1,
            employee_registry: self.core.employee_registry.clone(),
            metadata: serde_json::Value::Null,
        };

        // Prefer tools already instantiated by an active session so the
        // tool shares the session's ctx; fall back to creating one from
        // plugin registry by "pluginId.toolName".
        if let Some(tool) = self.find_tool_across_sessions(&name) {
            return tool.execute(&ctx, config).map_err(|e| e.to_string());
        }

        if let Some(dot_pos) = name.rfind('.') {
            let plugin_id = &name[..dot_pos];
            let tool_name = &name[dot_pos + 1..];
            if let Ok(tool) = self.create_tool(plugin_id, tool_name, ConfigValue::Null) {
                return tool.execute(&ctx, config).map_err(|e| e.to_string());
            }
        }

        Err(format!("tool '{}' not found", name))
    }

    fn call_llm(&mut self, _system: String, _user: String) -> Result<String, String> {
        Err("direct LLM calls not supported, drive a loop via sdk.loop(...).run()".to_string())
    }

    fn log(&mut self, level: String, message: String) {
        // Stderr prefix uses the display name so operators recognize
        // the employee; the persisted file lives under the immutable
        // slug so it shares the employee dir with .status/trigger/…
        eprintln!("[{}][{}] {}", self.employee_id, level, message);
        crate::sdk_log::record(&self.workforce_dir, &self.employee_slug, &level, &message);
    }

    fn get_context(&mut self, key: String) -> Option<String> {
        match key.as_str() {
            "workdir" => Some(
                self.workforce_dir
                    .join(&self.employee_id)
                    .to_string_lossy()
                    .to_string(),
            ),
            "employee" => Some(self.employee_id.clone()),
            "loop_id" => self.loop_stack.last().cloned(),
            _ => None,
        }
    }

    fn read_file(&mut self, path: String) -> Result<String, String> {
        std::fs::read_to_string(&path).map_err(|e| e.to_string())
    }

    fn write_file(&mut self, path: String, content: String) -> Result<(), String> {
        std::fs::write(&path, &content).map_err(|e| e.to_string())
    }

    fn loop_init(&mut self, config_json: String) -> Result<String, String> {
        let wasm_config: WasmLoopConfig =
            serde_json::from_str(&config_json).map_err(|e| format!("bad loop config: {e}"))?;

        let emp_id = self.employee_id.clone();
        let workforce_dir = self.workforce_dir.clone();
        let employee_registry = self.core.employee_registry.clone();

        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        if let Some(tool_defs) = &wasm_config.tools {
            for td in tool_defs {
                let config = json_to_config(&td.config);
                let tool = self.create_tool(&td.plugin, &td.tool, config)?;
                tools.push(tool);
            }
        }
        let mut tool_defs: Vec<ToolDef> = tools.iter().map(|t| to_tool_def(t.as_ref())).collect();

        // Structured-output opt-in: advertise a synthetic tool whose
        // schema IS the caller's desired final shape. The LLM calls it
        // to end the loop; we intercept in `loop_llm` and return its
        // args as `decision`. No real tool impl is registered — if the
        // SDK ever forwarded such a call via `loop_tool`, the missing
        // entry would surface the bug cleanly.
        let decision_tool: Option<String> = wasm_config.schema.as_ref().map(|s| {
            let name = s
                .name
                .clone()
                .unwrap_or_else(|| "record_decision".to_string());
            tool_defs.push(ToolDef {
                name: name.clone(),
                description: s
                    .description
                    .clone()
                    .unwrap_or_else(|| {
                        "Record the final structured decision for this loop. Call this exactly once as your last action.".to_string()
                    }),
                parameters_json: s.parameters_json.clone(),
            });
            name
        });

        let runner = match &wasm_config.runner {
            Some(rd) => {
                let config = json_to_config(&rd.config);
                self.create_runner(&rd.plugin_id, &rd.runner_id, config)?
            }
            None => return Err("no runner configured".to_string()),
        };

        let max_turns = runner
            .resolved_config
            .get("maxTurns")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_MAX_TURNS) as usize;
        let max_tokens = runner
            .resolved_config
            .get("maxTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_MAX_TOKENS) as usize;

        let llm = runner
            .plugin
            .build_backend(&runner.resolved_config)
            .map_err(|e| format!("build_backend failed: {e}"))?;

        // Filesystem (hold Arc so sandbox resources outlive the session)
        let (workdir, filesystem_arc) = if let Some(fs_def) = &wasm_config.filesystem {
            let fs_config = json_to_config(&fs_def.config);
            let fs = self.create_filesystem(&fs_def.plugin_id, &fs_def.fs_id, fs_config)?;
            let path = fs
                .resolve_workdir()
                .map_err(|e| format!("filesystem resolve failed: {e}"))?;
            (path, Some(fs))
        } else {
            (workforce_dir.join(&emp_id), None)
        };

        let session_id = format!(
            "{}_{}_{}",
            emp_id,
            wasm_config.name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );

        let ctx = LoopRunContext {
            id: session_id.clone(),
            employee: emp_id.clone(),
            loop_id: wasm_config.name.clone(),
            workforce_dir,
            workdir: workdir.clone(),
            max_turns,
            employee_registry,
            metadata: wasm_config.metadata.clone(),
        };

        let system_prompt = build_session_system_prompt(
            wasm_config.system.as_deref().unwrap_or(""),
            &tools,
            &workdir,
        );

        eprintln!(
            "[{}] loop_init '{}' (session={}, max_turns={}, tools={})",
            emp_id,
            wasm_config.name,
            session_id,
            max_turns,
            tools.len(),
        );

        self.loop_sessions.insert(
            session_id.clone(),
            LoopSession {
                tools,
                tool_defs,
                llm,
                ctx,
                system_prompt,
                max_tokens,
                loop_id: wasm_config.name.clone(),
                tool_call_seq: 0,
                _filesystem: filesystem_arc,
                decision_tool,
            },
        );
        self.loop_stack.push(wasm_config.name.clone());

        Ok(serde_json::json!({
            "id": session_id,
            "maxTurns": max_turns,
            "maxTokens": max_tokens,
        })
        .to_string())
    }

    fn loop_llm(&mut self, session_id: String, turns_json: String) -> Result<String, String> {
        let mut turns = parse_turns(&turns_json)?;
        let session = self
            .loop_sessions
            .get(&session_id)
            .ok_or_else(|| format!("unknown loop session '{session_id}'"))?;

        let request = LlmRequest {
            system_prompt: session.system_prompt.clone(),
            turns: turns.clone(),
            tools: session.tool_defs.clone(),
            max_tokens: session.max_tokens,
        };

        let mut response = session.llm.call(&request).map_err(|e| e.message)?;

        // Structured-output nudge. When a `schema` was configured and
        // the model tried to finish with plain text (no tool_use at
        // all — not even the decision tool), append an explicit user
        // nudge and retry ONE LLM call. The intermediate assistant
        // turn + nudge live only in this request's local `turns`; the
        // SDK's transcript picks up only the final response, so the
        // next turn isn't polluted by the non-compliance exchange.
        if let Some(decision_tool) = session.decision_tool.clone() {
            let had_decision = response
                .turn
                .blocks
                .iter()
                .any(|b| matches!(b, AssistantBlock::ToolUse { name, .. } if name == &decision_tool));
            let had_any_tool_use = response
                .turn
                .blocks
                .iter()
                .any(|b| matches!(b, AssistantBlock::ToolUse { .. }));

            if !had_decision && !had_any_tool_use {
                eprintln!(
                    "[{}][loop_llm] {} no decision tool call — nudging once",
                    session.ctx.employee, session.loop_id,
                );
                turns.push(Turn::Assistant(response.turn.clone()));
                turns.push(Turn::user_text(format!(
                    "Your answer must be recorded via the `{decision_tool}` tool, not as plain text. Call `{decision_tool}` now with your structured decision. No further reasoning — just the tool call."
                )));

                let retry_request = LlmRequest {
                    system_prompt: session.system_prompt.clone(),
                    turns,
                    tools: session.tool_defs.clone(),
                    max_tokens: session.max_tokens,
                };
                response = session.llm.call(&retry_request).map_err(|e| e.message)?;
            }
        }

        let usage = serde_json::json!({
            "inputTokens": response.input_tokens,
            "outputTokens": response.output_tokens,
            "cacheCreationInputTokens": response.cache_creation_input_tokens,
            "cacheReadInputTokens": response.cache_read_input_tokens,
        });

        // Persist per-turn usage for observability. One JSONL line per
        // LLM round-trip, written to <workforce_dir>/<employee>/usage.jsonl.
        // Cost estimate uses Anthropic's published Opus / Sonnet pricing
        // (current at 2026-04): Opus $15/M in, $75/M out, $18.75/M cache
        // write, $1.50/M cache read; Sonnet $3/$15/$3.75/$0.30.
        {
            let model = &response.model;
            let (in_per_m, out_per_m, cw_per_m, cr_per_m): (f64, f64, f64, f64) =
                if model.contains("opus") {
                    (15.0, 75.0, 18.75, 1.50)
                } else {
                    (3.0, 15.0, 3.75, 0.30)
                };
            let cost_usd = (response.input_tokens as f64 * in_per_m
                + response.output_tokens as f64 * out_per_m
                + response.cache_creation_input_tokens as f64 * cw_per_m
                + response.cache_read_input_tokens as f64 * cr_per_m)
                / 1_000_000.0;

            let usage_line = serde_json::json!({
                "ts": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                "loop_id": session.loop_id,
                "run_id": session.ctx.id,
                "model": model,
                "input_tokens": response.input_tokens,
                "output_tokens": response.output_tokens,
                "cache_creation_input_tokens": response.cache_creation_input_tokens,
                "cache_read_input_tokens": response.cache_read_input_tokens,
                "cost_usd": cost_usd,
            });
            let line = format!("{}\n", usage_line);
            let emp_dir = session.ctx.employee_dir();
            let path = emp_dir.join("usage.jsonl");
            // Best-effort append; observability should never kill a loop.
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
        }

        Ok(serialize_llm_response(
            &response.turn,
            usage,
            session.decision_tool.as_deref(),
        ))
    }

    fn loop_tool(
        &mut self,
        session_id: String,
        tool_name: String,
        args_json: String,
    ) -> Result<String, String> {
        let session = self
            .loop_sessions
            .get_mut(&session_id)
            .ok_or_else(|| format!("unknown loop session '{session_id}'"))?;

        let tool = session
            .tools
            .iter()
            .find(|t| t.name() == tool_name)
            .cloned()
            .ok_or_else(|| format!("tool '{tool_name}' not in session '{session_id}'"))?;

        // Count the call unconditionally — we track intent, not success.
        crate::stats::record(
            &session.ctx.workforce_dir,
            &session.ctx.employee,
            "tool",
            &tool_name,
        );

        session.tool_call_seq += 1;
        let turn = session.tool_call_seq;

        let args = match tool
            .parse(&args_json)
            .map_err(|e| format!("tool parse: {e}"))
        {
            Ok(a) => a,
            Err(e) => {
                // Parse failures are still audit-worthy.
                crate::stats::record_tool_call_detail(
                    &session.ctx.workforce_dir,
                    &session.ctx.employee,
                    &session.loop_id,
                    &session.ctx.id,
                    turn,
                    &tool_name,
                    &args_json,
                    &e,
                    true,
                    0,
                );
                return Err(e);
            }
        };

        let ctx = session.ctx_clone_for_call();
        let t0 = std::time::Instant::now();
        let outcome = tool.execute(&ctx, args);
        let duration_ms = t0.elapsed().as_millis();

        let (result_preview, is_error) = match &outcome {
            Ok(s) => (s.clone(), false),
            Err(e) => (format!("Tool error: {}", e.message), true),
        };

        crate::stats::record_tool_call_detail(
            &session.ctx.workforce_dir,
            &session.ctx.employee,
            &session.loop_id,
            &session.ctx.id,
            turn,
            &tool_name,
            &args_json,
            &result_preview,
            is_error,
            duration_ms,
        );

        outcome.map_err(|e| e.message)
    }

    fn loop_finish(&mut self, session_id: String) {
        if let Some(session) = self.loop_sessions.remove(&session_id) {
            eprintln!(
                "[{}] loop_finish '{}' (session={})",
                self.employee_id, session.loop_id, session_id,
            );
            // Pop the matching loop_id from the stack. Usually the top;
            // if not (nested-out-of-order, shouldn't happen), remove by
            // position to keep the stack consistent.
            if self.loop_stack.last().map(|s| s.as_str()) == Some(&session.loop_id) {
                self.loop_stack.pop();
            } else if let Some(pos) = self.loop_stack.iter().rposition(|s| s == &session.loop_id) {
                self.loop_stack.remove(pos);
            }
        }
    }

    fn subscribe(&mut self, name: String, config_json: String) {
        let config: serde_json::Value =
            serde_json::from_str(&config_json).unwrap_or(serde_json::Value::Null);

        let (plugin_id, trigger_name) = match name.rfind('.') {
            Some(pos) => (name[..pos].to_string(), name[pos + 1..].to_string()),
            None => (name.clone(), name.clone()),
        };

        eprintln!(
            "[{}][subscribe] {}.{} config={}",
            self.employee_id, plugin_id, trigger_name, config
        );

        self.subscriptions.push(Subscription {
            name: name.clone(),
            plugin_id,
            trigger_name,
            config: json_to_config(&config),
        });
    }
}

impl LoopSession {
    /// Fresh ctx clone for each tool call. `LoopRunContext` isn't Clone
    /// on purpose (some fields are heavyweight), but the fields we need
    /// are all cheap to share — Arcs, owned strings, owned paths.
    fn ctx_clone_for_call(&self) -> LoopRunContext {
        LoopRunContext {
            id: self.ctx.id.clone(),
            employee: self.ctx.employee.clone(),
            loop_id: self.ctx.loop_id.clone(),
            workforce_dir: self.ctx.workforce_dir.clone(),
            workdir: self.ctx.workdir.clone(),
            max_turns: self.ctx.max_turns,
            employee_registry: self.ctx.employee_registry.clone(),
            metadata: self.ctx.metadata.clone(),
        }
    }
}

// ── System prompt composition ──
//
// Mirrors the shape of loop_runner::build_system_prompt: user-supplied
// system text, plus each tool's `system_prompt()` fragment (deduped),
// plus a workdir listing. Lifetime fragments are intentionally absent —
// all lifetime/middleware work runs in TS and contributes nothing to
// the model-visible prompt here.

fn build_session_system_prompt(
    user_system: &str,
    tools: &[Arc<dyn Tool>],
    workdir: &Path,
) -> String {
    let mut prompt = user_system.to_string();

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut fragments: Vec<String> = Vec::new();
    for tool in tools {
        if let Some(frag) = tool.system_prompt() {
            let trimmed = frag.trim().to_string();
            if !trimmed.is_empty() && seen.insert(trimmed.clone()) {
                fragments.push(trimmed);
            }
        }
    }
    if !fragments.is_empty() {
        prompt.push_str("\n\n## Capability guidance\n");
        for frag in fragments {
            prompt.push('\n');
            prompt.push_str(&frag);
            prompt.push('\n');
        }
    }

    if let Ok(entries) = std::fs::read_dir(workdir) {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if entry.path().is_dir() {
                dirs.push(format!("{name}/"));
            } else {
                files.push(name);
            }
        }
        dirs.sort();
        files.sort();
        let mut listing = dirs;
        listing.extend(files);
        if !listing.is_empty() {
            prompt.push_str(&format!(
                "\n\nWorkdir: {}\nContents:\n{}",
                workdir.display(),
                listing
                    .iter()
                    .map(|e| format!("  {e}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ));
        }
    }

    prompt
}

// ── Public API ──

pub struct WasmEmployee {
    engine: Engine,
    component: Component,
    core: Arc<Core>,
    employee_id: String,
    workforce_dir: PathBuf,
}

impl WasmEmployee {
    /// Load a WASM employee config from a compiled .wasm file.
    pub fn load(
        wasm_path: &Path,
        core: Arc<Core>,
        employee_id: &str,
        workforce_dir: &Path,
    ) -> Result<Self, String> {
        let mut cfg = Config::new();
        cfg.wasm_component_model(true);
        let engine = Engine::new(&cfg).map_err(|e| format!("engine error: {}", e))?;
        let component =
            Component::from_file(&engine, wasm_path).map_err(|e| format!("load error: {}", e))?;

        Ok(Self {
            engine,
            component,
            core,
            employee_id: employee_id.to_string(),
            workforce_dir: workforce_dir.to_path_buf(),
        })
    }

    /// Read the employee's registration name from a freshly instantiated
    /// wasm module. Doesn't touch the registry — callers decide whether
    /// to insert/verify. Used by both `init()` (first-time registration)
    /// and `dispatch_instance()` (re-use registration that's already
    /// in the registry from the initial startup).
    fn instantiate_fresh(
        &self,
    ) -> Result<(Store<HostState>, LoopScript, EmployeeRegistration), String> {
        let mut linker = Linker::new(&self.engine);
        LoopScript::add_to_linker(&mut linker, |s| s)
            .map_err(|e| format!("linker error: {}", e))?;

        let state = HostState::new(self.core.clone(), &self.employee_id, &self.workforce_dir);
        let mut store = Store::new(&self.engine, state);
        let instance = LoopScript::instantiate(&mut store, &self.component, &linker)
            .map_err(|e| format!("instantiate error: {}", e))?;

        let reg_json = instance
            .call_get_registration(&mut store)
            .map_err(|e| format!("get_registration error: {}", e))?;
        if reg_json.is_empty() {
            return Err(format!(
                "WASM module at '{}' did not call sdk.register(...) — every employee WASM must register its identity at top level",
                self.employee_id
            ));
        }
        let registration: EmployeeRegistration = serde_json::from_str(&reg_json).map_err(|e| {
            format!(
                "invalid registration JSON from '{}': {}",
                self.employee_id, e
            )
        })?;
        if registration.name.trim().is_empty() {
            return Err(format!(
                "WASM module at '{}' registered with empty name",
                self.employee_id
            ));
        }
        store.data_mut().employee_id = registration.name.clone();
        Ok((store, instance, registration))
    }

    /// Initialize the employee: read registration, register in the global
    /// employee_registry, run `main`, collect subscriptions. Returns the
    /// list of subscriptions registered by the config.
    ///
    /// Note: jco uses Wizer to snapshot module init, so module top-level code
    /// runs at build time and CANNOT call host imports. Only `sdk.register(...)`
    /// (pure state) is safe at top level — all subscribe/call-tool work must
    /// live inside `export const main = () => {...}`, which the host invokes
    /// here at runtime.
    pub fn init(&self) -> Result<(Store<HostState>, LoopScript, Vec<Subscription>), String> {
        let (mut store, instance, registration) = self.instantiate_fresh()?;
        self.core
            .employee_registry
            .insert(registration)
            .map_err(|e| format!("employee registry: {}", e))?;
        instance
            .call_main(&mut store)
            .map_err(|e| format!("main error: {}", e))?;
        let subscriptions = std::mem::take(&mut store.data_mut().subscriptions);
        Ok((store, instance, subscriptions))
    }

    /// Create a **fresh** `(Store, Instance)` for driving one dispatch.
    /// Used by the multi-wasm dispatcher so long-running handlers
    /// (e.g. `msg_send(waitReply=true)` blocking on a human reply) don't
    /// serialize every other dispatch for the same employee.
    ///
    /// Runs `main()` to register per-instance handlers. Does NOT touch
    /// the global employee_registry — the caller is expected to have
    /// completed the one-time registration via `init()` at startup.
    pub fn dispatch_instance(&self) -> Result<(Store<HostState>, LoopScript), String> {
        let (mut store, instance, _registration) = self.instantiate_fresh()?;
        instance
            .call_main(&mut store)
            .map_err(|e| format!("main error: {}", e))?;
        // Discard the subscriptions list — watchers for this employee
        // were already spawned from the initial `init()` at startup.
        store.data_mut().subscriptions.clear();
        Ok((store, instance))
    }

    /// Dispatch an event to the WASM instance.
    pub fn dispatch(
        instance: &LoopScript,
        store: &mut Store<HostState>,
        handler: &str,
        data_json: &str,
    ) -> Result<(), String> {
        instance
            .call_dispatch(&mut *store, handler, data_json)
            .map_err(|e| format!("dispatch error: {}", e))?;
        Ok(())
    }
}
