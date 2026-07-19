//! One-shot dispatch: load an employee WASM, run `main()` to collect its
//! subscriptions, and deliver a single message into one handler — the
//! synchronous run path ported from the old workforce daemon's
//! `run_employee_loop`. The daemon (trigger watchers over persistent
//! queues) is future work; this entry point is what `adi-mono agents run`
//! uses for `wasm:*` backends today.

use std::path::Path;
use std::sync::Arc;

use crate::core::Core;
use crate::plugin::PluginError;
use crate::wasm_config_loader::WasmEmployee;

/// What a completed dispatch did, for display: which employee/handler ran
/// and the token usage its loops produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchOutcome {
    pub employee: String,
    /// The subscription name the message was delivered to.
    pub subscription: String,
    /// LLM round-trips recorded during the dispatch (0 when no loop ran).
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Deliver `message` to the employee compiled at `wasm_source`.
///
/// The wasm is installed at `<workforce_dir>/<id>/config.wasm` (id = file
/// stem) exactly like the old daemon's registration, `.env` files next to
/// the source are carried along and loaded into the process env, then a
/// fresh instance is dispatched once.
///
/// `handler` picks the subscription: exact trigger-name match, then a
/// fully-qualified `*.<handler>` suffix match, else the first subscription.
///
/// # Errors
/// Any load/init failure, an employee with no subscriptions, or a dispatch
/// error from inside the WASM.
pub fn dispatch_message(
    core: Arc<Core>,
    workforce_dir: &Path,
    wasm_source: &Path,
    handler: Option<&str>,
    message: &str,
) -> Result<DispatchOutcome, PluginError> {
    if !wasm_source.exists() {
        return Err(PluginError::new(format!(
            "wasm not found: {}",
            wasm_source.display()
        )));
    }

    let id = wasm_source
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Install into the workforce dir — same layout as the old daemon, so
    // logs/usage land under a stable per-employee directory.
    let dest_dir = workforce_dir.join(&id);
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| PluginError::new(format!("failed to create {}: {e}", dest_dir.display())))?;
    let dest_config = dest_dir.join("config.wasm");
    if wasm_source != dest_config {
        std::fs::copy(wasm_source, &dest_config)
            .map_err(|e| PluginError::new(format!("failed to install wasm: {e}")))?;
        if let Some(src_dir) = wasm_source.parent() {
            for env_name in &[".env", ".env.local"] {
                let env_src = src_dir.join(env_name);
                if env_src.is_file() {
                    let _ = std::fs::copy(&env_src, dest_dir.join(env_name));
                }
            }
        }
    }

    // Load .env into the process so runners can read API keys and the TS
    // side can resolve `functions.Env(...)` during main().
    load_env_files(&dest_dir);

    // Repeated dispatches with the same id must not fail with
    // "already registered".
    core.employee_registry.remove(&id);

    let emp = WasmEmployee::load(&dest_config, core, &id, workforce_dir)
        .map_err(|e| PluginError::new(format!("wasm load error: {e}")))?;
    let (mut store, instance, subscriptions) = emp
        .init()
        .map_err(|e| PluginError::new(format!("wasm init error: {e}")))?;

    if subscriptions.is_empty() {
        return Err(PluginError::new(
            "no subscriptions registered — the agent must subscribe at least one trigger \
             (e.g. orch.triggers.EmployeeMessage) for dispatch to have a target",
        ));
    }

    let sub = handler
        .and_then(|h| {
            subscriptions
                .iter()
                .find(|s| s.trigger_name == h || s.name.ends_with(&format!(".{h}")))
        })
        .or_else(|| subscriptions.first())
        .expect("non-empty subscriptions");

    let payload = serde_json::json!({ "from": "cli", "message": message }).to_string();

    // Usage attribution: the host appends one JSONL line per LLM round-trip
    // from inside loop_llm; sum only the lines added by this dispatch.
    let usage_path = workforce_dir.join(&id).join("usage.jsonl");
    let usage_offset = std::fs::metadata(&usage_path).map(|m| m.len()).unwrap_or(0);

    eprintln!(
        "[dispatch] '{}' → employee '{}' (subscriptions={})",
        sub.name,
        id,
        subscriptions.len(),
    );

    WasmEmployee::dispatch(&instance, &mut store, &sub.name, &payload)
        .map_err(|e| PluginError::new(format!("dispatch error: {e}")))?;

    let (turns, input_tokens, output_tokens) = aggregate_usage_since(&usage_path, usage_offset);

    Ok(DispatchOutcome {
        employee: id,
        subscription: sub.name.clone(),
        turns,
        input_tokens,
        output_tokens,
    })
}

/// Load simple `KEY=VALUE` lines from `.env` / `.env.local` in `dir` into the
/// process environment. Existing variables win — the shell environment stays
/// authoritative.
fn load_env_files(dir: &Path) {
    for name in &[".env", ".env.local"] {
        let Ok(content) = std::fs::read_to_string(dir.join(name)) else {
            continue;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !key.is_empty() && std::env::var_os(key).is_none() {
                // SAFETY-adjacent caveat: set_var is process-global; we only
                // call it during single-threaded dispatch setup.
                unsafe { std::env::set_var(key, value) };
            }
        }
    }
}

/// Sum `(lines, input_tokens, output_tokens)` from `usage.jsonl` content
/// appended after `offset` bytes.
fn aggregate_usage_since(path: &Path, offset: u64) -> (u64, u64, u64) {
    let Ok(content) = std::fs::read(path) else {
        return (0, 0, 0);
    };
    #[allow(clippy::cast_possible_truncation)]
    let tail = &content[content.len().min(offset as usize)..];
    let mut turns = 0u64;
    let mut input = 0u64;
    let mut output = 0u64;
    for line in String::from_utf8_lossy(tail).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        turns += 1;
        input += v
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        output += v
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
    }
    (turns, input, output)
}
